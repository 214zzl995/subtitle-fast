use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream::unfold};
use indicatif::{ProgressBar, ProgressStyle};
use tokio::sync::mpsc;

use super::StreamBundle;
use super::detector::{DetectionSample, DetectionSampleResult, DetectorError};

const PROGRESS_CHANNEL_CAPACITY: usize = 4;

pub struct Progress {
    label: &'static str,
}

impl Progress {
    pub fn new(label: &'static str) -> Self {
        Self { label }
    }

    pub fn attach(
        self,
        input: StreamBundle<DetectionSampleResult>,
    ) -> StreamBundle<DetectionSampleResult> {
        let StreamBundle {
            stream,
            total_frames,
        } = input;

        let (tx, rx) = mpsc::channel::<DetectionSampleResult>(PROGRESS_CHANNEL_CAPACITY);
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
        }
    }

    fn observe(&mut self, event: &DetectionSampleResult) {
        match event {
            Ok(candidate) => self.observe_sample(candidate),
            Err(err) => self.fail_with_reason(&describe_error(err)),
        }
    }

    fn observe_sample(&mut self, candidate: &DetectionSample) {
        self.samples_seen = self.samples_seen.saturating_add(1);
        if let Some(total) = self.total_frames {
            let frame_index = candidate.sample.frame_index();
            self.latest_frame_index = Some(frame_index);
            let next = std::cmp::min(frame_index.saturating_add(1), total);
            self.bar.set_position(next);
        } else {
            self.bar.inc(1);
        }
        self.observe_detection_time(candidate.elapsed);
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
            let avg = self
                .avg_detection_ms
                .map(|value| format!("{value:.1} ms"))
                .unwrap_or_else(|| "-- ms".to_string());
            self.bar
                .set_message(format!("{rate:.2}/s • detection: {avg}"));
        }
    }

    fn display_count(&self) -> u64 {
        self.latest_frame_index
            .map(|idx| idx.saturating_add(1))
            .unwrap_or(self.samples_seen)
    }
}

fn describe_error(err: &DetectorError) -> String {
    match err {
        DetectorError::Sampler(sampler_err) => format!("sampler error: {sampler_err}"),
        DetectorError::Detection(det_err) => format!("detector error: {det_err}"),
    }
}

fn bar_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{prefix:<10} {bar:40.cyan/blue} {percent:>3}% {pos}/{len} frames [{elapsed_precise}<{eta_precise}] speed {msg}",
    )
    .expect("invalid sampling bar template")
}

fn spinner_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{prefix:<10} {spinner:.cyan.bold} [{elapsed_precise}] frames {pos} • speed {msg}",
    )
    .expect("invalid sampling spinner template")
    .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
}
