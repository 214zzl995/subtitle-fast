use std::time::{Duration, Instant};

use indicatif::{ProgressBar, ProgressStyle};
use tokio_stream::StreamExt;

use subtitle_fast::{Configuration, YPlaneError};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), YPlaneError> {
    let config = Configuration::from_env().unwrap_or_default();
    let provider = config.create_provider()?;
    let mut stream = provider.into_stream();
    let mut emitted = 0usize;
    let progress = ProgressBar::new_spinner();
    progress.set_style(
        ProgressStyle::with_template("{spinner:.green} frame {pos} ×{msg}").unwrap(),
    );
    progress.enable_steady_tick(Duration::from_millis(100));
    progress.set_message("0.00");
    let mut last_multiplier = 0.0f64;
    let started_at = Instant::now();
    let mut first_timestamp: Option<Duration> = None;
    while let Some(frame) = stream.next().await {
        let frame = frame?;
        if first_timestamp.is_none() {
            first_timestamp = frame.timestamp();
        }
        progress.println(format!(
            "frame #{emitted}: {}x{} stride {} bytes {}",
            frame.width(),
            frame.height(),
            frame.stride(),
            frame.data().len()
        ));
        emitted += 1;
        let expected_elapsed = frame
            .timestamp()
            .and_then(|ts| first_timestamp.map(|first| ts.saturating_sub(first)))
            .map(|duration| duration.as_secs_f64())
            .unwrap_or_else(|| (emitted.saturating_sub(1) as f64) / 30.0);
        let actual_elapsed = started_at.elapsed().as_secs_f64().max(f64::MIN_POSITIVE);
        last_multiplier = if expected_elapsed <= f64::MIN_POSITIVE {
            0.0
        } else {
            expected_elapsed / actual_elapsed
        };
        progress.set_position(emitted as u64);
        progress.set_message(format!("{last_multiplier:.2}"));
        if emitted >= 5 {
            break;
        }
    }
    progress.finish_with_message(format!("{last_multiplier:.2}"));
    Ok(())
}
