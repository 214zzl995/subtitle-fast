use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::{Duration, Instant};

use indicatif::{ProgressBar, ProgressStyle};
use subtitle_fast_decoder::{Backend, Configuration, DynYPlaneProvider, YPlaneError};
use subtitle_fast_sink::{FrameSink, JpegOptions};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

fn usage() {
    println!("usage: subtitle-fast [--backend <name>] [--dump-dir <dir>] <video-path>");
    println!("       subtitle-fast --list-backends");
    print_available_backends();
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), YPlaneError> {
    let mut args = std::env::args_os();
    let _ = args.next();
    let mut backend_override = None;
    let mut input_path = None;
    let mut dump_dir: Option<PathBuf> = None;

    while let Some(arg) = args.next() {
        if arg == "--help" || arg == "-h" {
            usage();
            return Ok(());
        } else if arg == "--list-backends" {
            print_available_backends();
            return Ok(());
        } else if arg == "--backend" || arg == "-b" {
            let value = args
                .next()
                .ok_or_else(|| YPlaneError::configuration("--backend requires a backend name"))?;
            backend_override = Some(parse_backend(value)?);
        } else if let Some(value) = arg
            .to_str()
            .and_then(|s| s.strip_prefix("--backend="))
            .map(|s| s.to_owned())
        {
            backend_override = Some(parse_backend(OsString::from(value))?);
        } else if arg == "--dump-dir" {
            let value = args.next().ok_or_else(|| {
                YPlaneError::configuration("--dump-dir requires a directory path")
            })?;
            if dump_dir.is_some() {
                return Err(YPlaneError::configuration(
                    "--dump-dir specified more than once",
                ));
            }
            dump_dir = Some(PathBuf::from(value));
        } else if let Some(value) = arg
            .to_str()
            .and_then(|s| s.strip_prefix("--dump-dir="))
            .map(|s| s.to_owned())
        {
            if dump_dir.is_some() {
                return Err(YPlaneError::configuration(
                    "--dump-dir specified more than once",
                ));
            }
            dump_dir = Some(PathBuf::from(value));
        } else if input_path.is_none() {
            input_path = Some(PathBuf::from(arg));
        } else {
            return Err(YPlaneError::configuration(
                "multiple input paths provided; only one is supported",
            ));
        }
    }

    let input = match input_path {
        Some(path) => path,
        None => {
            usage();
            return Ok(());
        }
    };

    if let Some(dir) = dump_dir.as_ref() {
        fs::create_dir_all(dir)?;
    }

    let env_backend_present = std::env::var("SUBFAST_BACKEND").is_ok();
    let mut config = Configuration::from_env().unwrap_or_default();
    if let Some(backend) = backend_override {
        config.backend = backend;
    }
    config.input = Some(input);
    let backend_locked = backend_override.is_some() || env_backend_present;

    let available = Configuration::available_backends();
    if available.is_empty() {
        return Err(YPlaneError::configuration(
            "no decoding backend available; rebuild with a backend feature such as \"backend-ffmpeg\"",
        ));
    }
    if !available.contains(&config.backend) {
        return Err(YPlaneError::unsupported(config.backend.as_str()));
    }

    let available = Configuration::available_backends();
    let mut attempt_config = config.clone();
    let mut tried = Vec::new();

    loop {
        if !tried.contains(&attempt_config.backend) {
            tried.push(attempt_config.backend);
        }

        let provider = match attempt_config.create_provider() {
            Ok(provider) => provider,
            Err(err) => {
                if !backend_locked {
                    if let Some(next_backend) = next_backend(&available, &tried) {
                        let failed_backend = attempt_config.backend;
                        eprintln!(
                            "backend {failed} failed to initialize ({reason}); trying {next}",
                            failed = failed_backend.as_str(),
                            reason = err,
                            next = next_backend.as_str()
                        );
                        attempt_config.backend = next_backend;
                        continue;
                    }
                }
                return Err(err);
            }
        };

        match decode_with_provider(provider, dump_dir.clone()).await {
            Ok(()) => return Ok(()),
            Err((err, processed)) => {
                if processed == 0 && !backend_locked {
                    if let Some(next_backend) = next_backend(&available, &tried) {
                        let failed_backend = attempt_config.backend;
                        eprintln!(
                            "backend {failed} failed to decode ({reason}); trying {next}",
                            failed = failed_backend.as_str(),
                            reason = err,
                            next = next_backend.as_str()
                        );
                        attempt_config.backend = next_backend;
                        continue;
                    }
                }
                return Err(err);
            }
        }
    }
}

fn parse_backend(value: OsString) -> Result<Backend, YPlaneError> {
    let value = value
        .into_string()
        .map_err(|_| YPlaneError::configuration("backend name must be valid UTF-8"))?;
    Backend::from_str(&value)
}

async fn decode_with_provider(
    provider: DynYPlaneProvider,
    dump_dir: Option<PathBuf>,
) -> Result<(), (YPlaneError, u64)> {
    let total_frames = provider.total_frames();
    let mut stream = provider.into_stream();

    let progress = match total_frames {
        Some(total) => {
            let bar = ProgressBar::new(total);
            bar.set_style(
                ProgressStyle::with_template(
                    "{bar:40.cyan/blue} {percent:>3}% {pos}/{len} frames [{elapsed_precise}<{eta_precise}] speed {msg}",
                )
                .unwrap(),
            );
            bar
        }
        None => {
            let spinner = ProgressBar::new_spinner();
            spinner.set_style(
                ProgressStyle::with_template(
                    "{spinner:.cyan.bold} [{elapsed_precise}] frames {pos} • speed {msg}",
                )
                .unwrap()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
            );
            spinner
        }
    };
    progress.enable_steady_tick(Duration::from_millis(100));

    let mut processed: u64 = 0;
    let started = Instant::now();
    let frame_sink = dump_dir.map(|dir| FrameSink::jpeg_writer(dir, JpegOptions::default()));
    let progress_capacity = progress_channel_capacity(total_frames);
    let (progress_tx, progress_rx) = mpsc::channel::<ProgressEvent>(progress_capacity);
    let progress_task = tokio::spawn(drive_progress(
        progress.clone(),
        progress_rx,
        total_frames,
        started,
    ));

    let mut failure: Option<YPlaneError> = None;

    while let Some(frame) = stream.next().await {
        match frame {
            Ok(frame) => {
                processed = processed.saturating_add(1);
                let timestamp = frame.timestamp();

                if let Some(sink) = frame_sink.as_ref() {
                    let frame_index = frame
                        .frame_index()
                        .unwrap_or_else(|| processed.saturating_sub(1));
                    let _ = sink.push(frame.clone(), frame_index);
                }

                let event = ProgressEvent {
                    index: processed,
                    timestamp,
                };
                if let Err(err) = progress_tx.try_send(event) {
                    let event = err.into_inner();
                    if progress_tx.send(event).await.is_err() {
                        break;
                    }
                }
            }
            Err(err) => {
                failure = Some(err);
                break;
            }
        }
    }

    drop(progress_tx);
    let summary = progress_task.await.expect("progress task panicked");

    if let Some(err) = failure {
        let processed = summary.processed;
        progress.abandon_with_message(format!("failed after {processed} frames"));
        return Err((err, processed));
    }

    if let Some(sink) = frame_sink {
        sink.shutdown().await;
    }

    if let Some(total) = total_frames {
        let display_total = if summary.processed < total {
            progress.set_length(summary.processed);
            summary.processed
        } else {
            total
        };
        if summary.processed >= display_total {
            progress.set_position(display_total);
        }
        progress.finish_with_message(format!(
            "completed {}/{} frames @ {:.2}x",
            summary.processed, display_total, summary.last_speed
        ));
    } else {
        progress.finish_with_message(format!(
            "completed {} frames @ {:.2}x",
            summary.processed, summary.last_speed
        ));
    }

    Ok(())
}

fn next_backend(available: &[Backend], tried: &[Backend]) -> Option<Backend> {
    available
        .iter()
        .copied()
        .find(|backend| !tried.contains(backend))
}

fn print_available_backends() {
    let names: Vec<&'static str> = Configuration::available_backends()
        .iter()
        .map(Backend::as_str)
        .collect();
    if names.is_empty() {
        println!("available backends: (none compiled)");
    } else {
        println!("available backends: {}", names.join(", "));
    }
}

#[derive(Debug)]
struct ProgressEvent {
    index: u64,
    timestamp: Option<Duration>,
}

#[derive(Debug)]
struct ProgressSummary {
    processed: u64,
    last_speed: f64,
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
        processed = event.index;

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
