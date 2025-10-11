use std::time::{Duration, Instant};

use indicatif::{ProgressBar, ProgressStyle};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

#[derive(Debug, Clone)]
pub struct ProgressEvent {
    pub index: u64,
    pub timestamp: Option<Duration>,
}

#[derive(Debug, Clone)]
pub struct ProgressSummary {
    pub processed: u64,
    pub last_speed: f64,
}

pub fn start_progress(
    label: impl Into<String>,
    total_frames: Option<u64>,
    started: Instant,
) -> (
    ProgressBar,
    mpsc::Sender<ProgressEvent>,
    JoinHandle<ProgressSummary>,
) {
    let label = label.into();
    let progress_bar = create_progress_bar(&label, total_frames);
    progress_bar.enable_steady_tick(Duration::from_millis(100));

    let capacity = progress_channel_capacity(total_frames);
    let (tx, rx) = mpsc::channel::<ProgressEvent>(capacity);
    let task = tokio::spawn(drive_progress(
        progress_bar.clone(),
        rx,
        total_frames,
        started,
    ));

    (progress_bar, tx, task)
}

pub fn finalize_success(bar: &ProgressBar, summary: &ProgressSummary, total_frames: Option<u64>) {
    if let Some(total) = total_frames {
        let display_total = if summary.processed < total {
            bar.set_length(summary.processed);
            summary.processed
        } else {
            total
        };
        if summary.processed >= display_total {
            bar.set_position(display_total);
        }
        bar.finish_with_message(format!(
            "completed {}/{} frames @ {:.2}x",
            summary.processed, display_total, summary.last_speed
        ));
    } else {
        bar.finish_with_message(format!(
            "completed {} frames @ {:.2}x",
            summary.processed, summary.last_speed
        ));
    }
}

fn create_progress_bar(label: &str, total_frames: Option<u64>) -> ProgressBar {
    match total_frames {
        Some(total) => {
            let bar = ProgressBar::new(total);
            bar.set_style(
                ProgressStyle::with_template(
                    "{prefix:<10} {bar:40.cyan/blue} {percent:>3}% {pos}/{len} frames [{elapsed_precise}<{eta_precise}] speed {msg}",
                )
                .unwrap(),
            );
            bar.set_prefix(label.to_string());
            bar
        }
        None => {
            let spinner = ProgressBar::new_spinner();
            spinner.set_style(
                ProgressStyle::with_template(
                    "{prefix:<10} {spinner:.cyan.bold} [{elapsed_precise}] frames {pos} • speed {msg}",
                )
                .unwrap()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
            );
            spinner.set_prefix(label.to_string());
            spinner
        }
    }
}

fn progress_channel_capacity(total_frames: Option<u64>) -> usize {
    match total_frames {
        Some(total) => total.min(1024).max(64).try_into().unwrap_or(1024),
        None => 512,
    }
}

async fn drive_progress(
    progress: ProgressBar,
    mut rx: mpsc::Receiver<ProgressEvent>,
    total_frames: Option<u64>,
    started: Instant,
) -> ProgressSummary {
    let mut processed = 0u64;
    let mut last_speed = 0.0f64;

    while let Some(event) = rx.recv().await {
        processed = processed.max(event.index);

        if let Some(total) = total_frames {
            if processed > total {
                progress.set_length(processed);
            }
        }

        progress.set_position(processed);

        let media_position = event
            .timestamp
            .unwrap_or_else(|| Duration::from_secs_f64(processed as f64 / 30.0));
        let elapsed_secs = started.elapsed().as_secs_f64();
        if elapsed_secs > 0.0 {
            last_speed = media_position.as_secs_f64() / elapsed_secs;
            progress.set_message(format!("{:.2}x", last_speed));
        }
    }

    ProgressSummary {
        processed,
        last_speed,
    }
}
