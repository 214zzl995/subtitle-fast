use std::path::PathBuf;
use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream::unfold};
use indicatif::{ProgressBar, ProgressStyle};
use tokio::sync::mpsc;

use super::StreamBundle;
use super::detector::DetectionSample;
use super::lifecycle::RegionTimings;
use super::ocr::{OcrStageError, OcrTimings};
use super::writer::{SubtitleWriterError, WriterResult, WriterStatus, WriterTimings};

const PROGRESS_CHANNEL_CAPACITY: usize = 4;
const COL_AVG: &str = "\x1b[33m"; // yellow-ish for averages
const COL_COUNT: &str = "\x1b[36m"; // cyan-ish for counts
const COL_RESET: &str = "\x1b[0m";

pub struct Progress {
    label: &'static str,
}

impl Progress {
    pub fn new(label: &'static str) -> Self {
        Self { label }
    }

    pub fn attach(self, input: StreamBundle<WriterResult>) -> StreamBundle<WriterResult> {
        let StreamBundle {
            stream,
            total_frames,
        } = input;

        let (tx, rx) = mpsc::channel::<WriterResult>(PROGRESS_CHANNEL_CAPACITY);
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
            receiver.recv().await.map(|item| (item, receiver))
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
    region_frames: u64,
    region_total: Duration,
    ocr_intervals: u64,
    ocr_total: Duration,
    writer_cues: u64,
    writer_merged: u64,
    writer_empty_ocr: u64,
    writer_total: Duration,
    completed_output: Option<(PathBuf, usize)>,
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
            region_frames: 0,
            region_total: Duration::ZERO,
            ocr_intervals: 0,
            ocr_total: Duration::ZERO,
            writer_cues: 0,
            writer_merged: 0,
            writer_empty_ocr: 0,
            writer_total: Duration::ZERO,
            completed_output: None,
        }
    }

    fn observe(&mut self, event: &WriterResult) {
        match event {
            Ok(event) => {
                if let Some(sample) = &event.sample {
                    self.observe_sample(sample);
                }
                self.observe_region_time(event.region_timings);
                self.observe_ocr_time(event.ocr_timings);
                let completed = matches!(event.status, WriterStatus::Completed { .. });
                self.observe_writer_time(event.writer_timings, completed);
                if let WriterStatus::Completed { path, cues } = &event.status {
                    self.completed_output = Some((path.clone(), *cues));
                }
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

    fn observe_region_time(&mut self, timings: Option<RegionTimings>) {
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

    fn observe_writer_time(&mut self, timings: Option<WriterTimings>, completed: bool) {
        let Some(timings) = timings else {
            return;
        };
        if completed {
            self.writer_cues = timings.cues;
            self.writer_merged = timings.merged;
        } else if timings.cues > 0 {
            self.writer_cues = self.writer_cues.saturating_add(timings.cues);
        }
        if !completed {
            self.writer_merged = self.writer_merged.saturating_add(timings.merged);
        }
        self.writer_empty_ocr = self.writer_empty_ocr.saturating_add(timings.ocr_empty);
        self.writer_total = self.writer_total.saturating_add(timings.total);
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
        if let Some(total) = self.total_frames {
            self.bar.set_position(total);
        }
        if let Some((path, cues)) = &self.completed_output {
            let processed_line = match self.total_frames {
                Some(total) => format!("processed {}/{} frames", total, total),
                None => format!("processed {} frames", self.display_count()),
            };
            let output_line = format!(
                "wrote {} ({} cues, merged {}, ocr-empty {})",
                path.display(),
                cues,
                self.writer_merged,
                self.writer_empty_ocr
            );
            let det = self
                .avg_detection_ms
                .map(|value| format!("{value:.1} ms"))
                .unwrap_or_else(|| "-- ms".to_string());
            let reg = average_ms(self.region_total, self.region_frames);
            let ocr = average_ms(self.ocr_total, self.ocr_intervals);
            let writer = average_ms(self.writer_total, self.writer_cues);
            let counts_line = format!(
                "[{COL_COUNT}counts{COL_RESET}] cues {} • merged {} • ocr-empty {}",
                self.writer_cues, self.writer_merged, self.writer_empty_ocr
            );
            let avg_line = format!(
                "[{COL_AVG}   avg{COL_RESET}] det {det} • reg {reg} • ocr {ocr} • wr {writer}"
            );
            let summary = format!("{processed_line}\n{output_line}\n{avg_line}\n{counts_line}");
            self.bar.finish_with_message(summary);
        } else {
            match self.total_frames {
                Some(total) => {
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
    }

    fn update_speed(&self) {
        if self.started.elapsed().as_secs_f64() <= 0.0 {
            return;
        }

        let units = self
            .latest_frame_index
            .map(|idx| idx.saturating_add(1) as f64)
            .unwrap_or(self.samples_seen as f64);
        let rate = units / self.started.elapsed().as_secs_f64();

        let det = self
            .avg_detection_ms
            .map(|value| format!("{value:.1} ms"))
            .unwrap_or_else(|| "-- ms".to_string());
        let reg = average_ms(self.region_total, self.region_frames);
        let ocr = average_ms(self.ocr_total, self.ocr_intervals);
        let cues = self.writer_cues;
        let merged = self.writer_merged;
        let writer = average_ms(self.writer_total, self.writer_cues);

        let avg_line = format!(
            "[{COL_AVG}   avg{COL_RESET}] fps {rate:>5.1} • det {det} • reg {reg} • ocr {ocr} • wr {writer}"
        );
        let counts_line = format!(
            "[{COL_COUNT}counts{COL_RESET}] cues {cues} • merged {merged} • ocr-empty {}",
            self.writer_empty_ocr
        );
        self.bar.set_message(format!("{avg_line}\n{counts_line}"));
    }

    fn display_count(&self) -> u64 {
        self.latest_frame_index
            .map(|idx| idx.saturating_add(1))
            .unwrap_or(self.samples_seen)
    }
}

fn average_ms(total: Duration, units: u64) -> String {
    if units == 0 {
        return "-- ms".into();
    }
    let avg_ms = total.as_secs_f64() * 1000.0 / units as f64;
    format!("{avg_ms:.1} ms")
}

fn describe_error(err: &SubtitleWriterError) -> String {
    match err {
        SubtitleWriterError::Ocr(ocr_err) => describe_ocr_error(ocr_err),
        SubtitleWriterError::Io { path, source } => {
            format!("writer error ({}): {source}", path.display())
        }
    }
}

fn describe_ocr_error(err: &OcrStageError) -> String {
    match err {
        super::ocr::OcrStageError::Lifecycle(lifecycle_err) => match lifecycle_err {
            super::lifecycle::RegionLifecycleError::Determiner(det_err) => match det_err {
                super::determiner::RegionDeterminerError::Detector(detector_err) => {
                    match detector_err {
                        super::detector::DetectorError::Sampler(sampler_err) => {
                            format!("sampler error: {sampler_err}")
                        }
                        super::detector::DetectorError::Detection(det_err) => {
                            format!("detector error: {det_err}")
                        }
                    }
                }
            },
        },
        super::ocr::OcrStageError::Engine(engine_err) => {
            format!("ocr error: {engine_err}")
        }
    }
}

fn bar_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{prefix:.bold} {bar:40.cyan/blue} {percent:>3.bold}% {pos:>5}/{len:<5} [{elapsed_precise:.dim}<{eta_precise:.dim}]\n{msg}",
    )
    .expect("invalid sampling bar template")
    .progress_chars("█▉▊▋▌▍▎▏ ")
}

fn spinner_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{prefix:.bold} {spinner:.cyan.bold} [{elapsed_precise:.dim}] {pos:>5}f\n{msg}",
    )
    .expect("invalid sampling spinner template")
    .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
}
