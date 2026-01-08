use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream::unfold};
use tokio::sync::mpsc;

use super::StreamBundle;
use super::merge::{MergeOutput, MergeResult};
use super::ocr::OcrTimings;
use super::{PipelineError, PipelineProgress, PipelineUpdate};

const AVERAGER_CHANNEL_CAPACITY: usize = 4;
const EMA_ALPHA: f64 = 0.1;

pub type AveragerResult = Result<PipelineUpdate, PipelineError>;

pub struct Averager;

impl Averager {
    pub fn new() -> Self {
        Self
    }

    pub fn attach(self, input: StreamBundle<MergeResult>) -> StreamBundle<AveragerResult> {
        let StreamBundle {
            stream,
            total_frames,
        } = input;

        let (tx, rx) = mpsc::channel::<AveragerResult>(AVERAGER_CHANNEL_CAPACITY);

        tokio::spawn(async move {
            let mut upstream = stream;
            let mut state = AveragerState::new(total_frames);

            while let Some(event) = upstream.next().await {
                match event {
                    Ok(output) => {
                        state.observe(&output);
                        let snapshot = state.snapshot(false);
                        let update = PipelineUpdate {
                            progress: snapshot,
                            updates: output.updates,
                        };
                        if tx.send(Ok(update)).await.is_err() {
                            return;
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(Err(PipelineError::Ocr(err))).await;
                        return;
                    }
                }
            }

            let _ = tx
                .send(Ok(PipelineUpdate {
                    progress: state.snapshot(true),
                    updates: Vec::new(),
                }))
                .await;
        });

        let stream = Box::pin(unfold(rx, |mut receiver| async {
            receiver.recv().await.map(|item| (item, receiver))
        }));

        StreamBundle::new(stream, total_frames)
    }
}

struct AveragerState {
    total_frames: Option<u64>,
    samples_seen: u64,
    latest_frame_index: Option<u64>,
    started: Instant,
    avg_detection_ms: Option<f64>,
    region_frames: u64,
    region_total: Duration,
    ocr_intervals: u64,
    ocr_total: Duration,
    cues: u64,
    merged: u64,
    ocr_empty: u64,
}

impl AveragerState {
    fn new(total_frames: Option<u64>) -> Self {
        Self {
            total_frames,
            samples_seen: 0,
            latest_frame_index: None,
            started: Instant::now(),
            avg_detection_ms: None,
            region_frames: 0,
            region_total: Duration::ZERO,
            ocr_intervals: 0,
            ocr_total: Duration::ZERO,
            cues: 0,
            merged: 0,
            ocr_empty: 0,
        }
    }

    fn observe(&mut self, event: &MergeOutput) {
        if let Some(sample) = &event.sample {
            self.samples_seen = self.samples_seen.saturating_add(1);
            if let Some(total) = self.total_frames {
                let frame_index = sample.sample.frame_index();
                self.latest_frame_index = Some(frame_index);
                self.samples_seen = std::cmp::min(frame_index.saturating_add(1), total);
            }
            self.observe_detection_time(sample.elapsed);
        }

        self.observe_region_time(event.region_timings);
        self.observe_ocr_time(event.ocr_timings);
        self.cues = event.stats.cues;
        self.merged = event.stats.merged;
        self.ocr_empty = event.stats.ocr_empty;
    }

    fn observe_detection_time(&mut self, elapsed: Duration) {
        let millis = elapsed.as_secs_f64() * 1000.0;
        self.avg_detection_ms = Some(match self.avg_detection_ms {
            Some(current) => (1.0 - EMA_ALPHA) * current + EMA_ALPHA * millis,
            None => millis,
        });
    }

    fn observe_region_time(&mut self, timings: Option<super::lifecycle::RegionTimings>) {
        let Some(timings) = timings else {
            return;
        };
        self.region_frames = self.region_frames.saturating_add(timings.frames);
        self.region_total = self.region_total.saturating_add(timings.total);
    }

    fn observe_ocr_time(&mut self, timings: Option<OcrTimings>) {
        let Some(timings) = timings else {
            return;
        };
        self.ocr_intervals = self.ocr_intervals.saturating_add(timings.intervals);
        self.ocr_total = self.ocr_total.saturating_add(timings.total);
    }

    fn snapshot(&self, completed: bool) -> PipelineProgress {
        let latest = self.latest_frame_index.unwrap_or(self.samples_seen);
        let elapsed = self.started.elapsed().as_secs_f64();
        PipelineProgress {
            samples_seen: self.samples_seen,
            latest_frame_index: latest,
            total_frames: self.total_frames,
            fps: if elapsed > 0.0 {
                (latest as f64) / elapsed
            } else {
                0.0
            },
            det_ms: self.avg_detection_ms.unwrap_or(0.0),
            seg_ms: average_ms(self.region_total, self.region_frames),
            ocr_ms: average_ms(self.ocr_total, self.ocr_intervals),
            cues: self.cues,
            merged: self.merged,
            ocr_empty: self.ocr_empty,
            progress: if let Some(total) = self.total_frames {
                if total > 0 {
                    (latest as f64) / (total as f64)
                } else {
                    0.0
                }
            } else {
                0.0
            },
            completed,
        }
    }
}

fn average_ms(total: Duration, units: u64) -> f64 {
    if units == 0 {
        return 0.0;
    }
    total.as_secs_f64() * 1000.0 / units as f64
}
