use std::path::PathBuf;
use std::str::FromStr;
use std::time::Instant;
use std::{ffi::OsString, time::Duration};

use indicatif::{ProgressBar, ProgressStyle};
use subtitle_fast_decoder::{Backend, Configuration, DynYPlaneProvider, YPlaneError};
use tokio_stream::StreamExt;

fn usage() {
    println!("usage: subtitle-fast [--backend <name>] <video-path>");
    println!("       subtitle-fast --list-backends");
    print_available_backends();
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), YPlaneError> {
    let mut args = std::env::args_os();
    let _ = args.next();
    let mut backend_override = None;
    let mut input_path = None;

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

    let mut config = Configuration::from_env().unwrap_or_default();
    if let Some(backend) = backend_override {
        config.backend = backend;
    }
    config.input = Some(input);

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
                if let Some(next_backend) =
                    determine_fallback_backend(attempt_config.backend, &available, &tried, &err)
                {
                    attempt_config.backend = next_backend;
                    continue;
                } else {
                    return Err(err);
                }
            }
        };

        match decode_with_provider(provider).await {
            Ok(()) => return Ok(()),
            Err((err, processed)) => {
                if processed == 0 {
                    if let Some(next_backend) =
                        determine_fallback_backend(attempt_config.backend, &available, &tried, &err)
                    {
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

async fn decode_with_provider(provider: DynYPlaneProvider) -> Result<(), (YPlaneError, u64)> {
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
    let mut last_timestamp: Option<Duration> = None;
    let mut last_speed = 0.0f64;
    let started = Instant::now();

    while let Some(frame) = stream.next().await {
        match frame {
            Ok(frame) => {
                processed += 1;
                if let Some(ts) = frame.timestamp() {
                    last_timestamp = Some(ts);
                }

                let elapsed = started.elapsed();
                let media_position = last_timestamp
                    .unwrap_or_else(|| Duration::from_secs_f64(processed as f64 / 30.0));
                let elapsed_secs = elapsed.as_secs_f64();
                if elapsed_secs > 0.0 {
                    last_speed = media_position.as_secs_f64() / elapsed_secs;
                }

                if let Some(total) = total_frames {
                    if processed > total {
                        progress.set_length(processed);
                    }
                }
                progress.set_position(processed);
                progress.set_message(format!("{:.2}x", last_speed));
            }
            Err(err) => {
                progress.abandon_with_message(format!("failed after {processed} frames"));
                return Err((err, processed));
            }
        }
    }

    if let Some(total) = total_frames {
        let display_total = if processed < total {
            progress.set_length(processed);
            progress.set_position(processed);
            processed
        } else {
            total
        };
        if processed >= display_total {
            progress.set_position(display_total);
        }
        progress.finish_with_message(format!(
            "completed {processed}/{display_total} frames @ {:.2}x",
            last_speed
        ));
    } else {
        progress.finish_with_message(format!("completed {processed} frames @ {:.2}x", last_speed));
    }

    Ok(())
}

fn determine_fallback_backend(
    current: Backend,
    available: &[Backend],
    tried: &[Backend],
    err: &YPlaneError,
) -> Option<Backend> {
    if current != Backend::VideoToolbox {
        return None;
    }
    let message = match err {
        YPlaneError::BackendFailure { backend, message } if *backend == "videotoolbox" => message,
        _ => return None,
    };
    if !message.contains("Cannot Decode") {
        return None;
    }
    #[cfg(feature = "backend-ffmpeg")]
    {
        if available.contains(&Backend::Ffmpeg) && !tried.contains(&Backend::Ffmpeg) {
            eprintln!("videotoolbox backend cannot decode this media; falling back to ffmpeg");
            return Some(Backend::Ffmpeg);
        }
    }
    #[cfg(not(feature = "backend-ffmpeg"))]
    {
        let _ = (available, tried);
    }
    None
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
