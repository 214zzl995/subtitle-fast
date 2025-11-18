use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream::unfold};
use indicatif::{ProgressBar, ProgressStyle};
use tokio::sync::mpsc;

use super::StreamBundle;
use super::detector::{DetectionSample, DetectorError};
use super::segmenter::{SegmenterError, SegmenterResult};

const PROGRESS_CHANNEL_CAPACITY: usize = 4;

pub struct Progress {
    label: &'static str,
}

impl Progress {
    pub fn new(label: &'static str) -> Self {
        Self { label }
    }

    pub fn attach(self, input: StreamBundle<SegmenterResult>) -> StreamBundle<SegmenterResult> {
        let StreamBundle {
            stream,
            total_frames,
        } = input;

        let (tx, rx) = mpsc::channel::<SegmenterResult>(PROGRESS_CHANNEL_CAPACITY);
        let label = self.label;

        tokio::spawn(async move {
            let mut upstream = stream;
            let mut monitor = ProgressMonitor::new(label, total_frames);

            while let Some(event) = upstream.next().await {
                monitor.observe(&event);
                if tx.send(event).await.is_err() {
                    monitor.finish_if_needed();
                    return;
                }
            }

            monitor.finish_if_needed();
        });

        let stream = Box::pin(unfold(rx, |mut receiver| async {
            match receiver.recv().await {
                Some(item) => Some((item, receiver)),
                None => None,
            }
        }));

        StreamBundle::new(stream, total_frames)
    }
}

struct ProgressMonitor {
    bar: ProgressBar,
    total_frames: Option<u64>,
    samples_seen: u64,
    latest_frame_index: Option<u64>,
    started: Instant,
    finished: bool,
    avg_detection_ms: Option<f64>,
    avg_segment_ms: Option<f64>,
}

impl ProgressMonitor {
    fn new(label: &'static str, total_frames: Option<u64>) -> Self {
        let bar = match total_frames {
            Some(total) => {
                let bar = ProgressBar::new(total);
                bar.set_style(bar_style());
                bar
            }
            None => {
                let bar = ProgressBar::new_spinner();
                bar.set_style(spinner_style());
                bar
            }
        };
        bar.set_prefix(label);

        Self {
            bar,
            total_frames,
            samples_seen: 0,
            latest_frame_index: None,
            started: Instant::now(),
            finished: false,
            avg_detection_ms: None,
            avg_segment_ms: None,
        }
    }

    fn observe(&mut self, event: &SegmenterResult) {
        match event {
            Ok(event) => {
                if let Some(sample) = &event.sample {
                    self.observe_sample(sample);
                }
                self.observe_segment_time(event.segment_elapsed);
            }
            Err(err) => self.fail_with_reason(&describe_error(err)),
        }
    }

    fn observe_sample(&mut self, sample: &DetectionSample) {
        self.samples_seen = self.samples_seen.saturating_add(1);
        if let Some(total) = self.total_frames {
            let frame_index = sample.sample.frame_index();
            self.latest_frame_index = Some(frame_index);
            let next = std::cmp::min(frame_index.saturating_add(1), total);
            self.bar.set_position(next);
        } else {
            self.bar.inc(1);
        }
        self.observe_detection_time(sample.elapsed);
        self.update_speed();
    }

    fn observe_detection_time(&mut self, elapsed: Duration) {
        let millis = elapsed.as_secs_f64() * 1000.0;
        let alpha = 0.1;
        self.avg_detection_ms = Some(match self.avg_detection_ms {
            Some(current) => (1.0 - alpha) * current + alpha * millis,
            None => millis,
        });
    }

    fn observe_segment_time(&mut self, elapsed: Option<Duration>) {
        let Some(elapsed) = elapsed else {
            return;
        };
        let millis = elapsed.as_secs_f64() * 1000.0;
        let alpha = 0.1;
        self.avg_segment_ms = Some(match self.avg_segment_ms {
            Some(current) => (1.0 - alpha) * current + alpha * millis,
            None => millis,
        });
    }

    fn fail_with_reason(&mut self, reason: &str) {
        if self.finished {
            return;
        }
        self.finished = true;
        if let Some(total) = self.total_frames {
            let pos = std::cmp::min(self.display_count(), total);
            self.bar.set_position(pos);
        }
        self.bar.abandon_with_message(format!(
            "failed after {} frames: {reason}",
            self.display_count()
        ));
    }

    fn finish_if_needed(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        match self.total_frames {
            Some(total) => {
                self.bar.set_position(total);
                self.bar
                    .finish_with_message(format!("processed {}/{} frames", total, total));
            }
            None => {
                let processed = self.display_count();
                self.bar
                    .finish_with_message(format!("processed {processed} frames"));
            }
        }
    }

    fn update_speed(&self) {
        let elapsed = self.started.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            let units = self
                .latest_frame_index
                .map(|idx| idx.saturating_add(1) as f64)
                .unwrap_or(self.samples_seen as f64);
            let rate = units / elapsed;
            let det = self
                .avg_detection_ms
                .map(|value| format!("{value:.1} ms"))
                .unwrap_or_else(|| "-- ms".to_string());
            let seg = self
                .avg_segment_ms
                .map(|value| format!("{value:.1} ms"))
                .unwrap_or_else(|| "-- ms".to_string());
            self.bar
                .set_message(format!("{rate:.1} fps • det {det} • seg {seg}"));
        }
    }

    fn display_count(&self) -> u64 {
        self.latest_frame_index
            .map(|idx| idx.saturating_add(1))
            .unwrap_or(self.samples_seen)
    }
}

fn describe_error(err: &SegmenterError) -> String {
    match err {
        SegmenterError::Detector(detector_err) => match detector_err {
            DetectorError::Sampler(sampler_err) => format!("sampler error: {sampler_err}"),
            DetectorError::Detection(det_err) => format!("detector error: {det_err}"),
        },
    }
}

fn bar_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{prefix:.bold} {bar:40.cyan/blue} {percent:>3.bold}% {pos:>5}/{len:<5} [{elapsed_precise:.dim}<{eta_precise:.dim}] {msg:.yellow}",
    )
    .expect("invalid sampling bar template")
    .progress_chars("█▉▊▋▌▍▎▏ ")
}

fn spinner_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{prefix:.bold} {spinner:.cyan.bold} [{elapsed_precise:.dim}] {pos:>5}f {msg:.yellow}",
    )
    .expect("invalid sampling spinner template")
    .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
}
