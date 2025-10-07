use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Instant;

mod cli;
mod progress;

use clap::{CommandFactory, Parser};
use cli::{CliArgs, DumpFormat};
use progress::{ProgressEvent, finalize_success, start_progress};
use subtitle_fast_decoder::{Backend, Configuration, DynYPlaneProvider, YPlaneError};
use subtitle_fast_sink::{
    FrameMetadata, FrameSink, FrameSinkConfig, FrameSinkError, ImageOutputFormat,
};
use tokio_stream::StreamExt;

fn usage() {
    let mut command = CliArgs::command();
    command.print_help().ok();
    println!();
    display_available_backends();
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), YPlaneError> {
    let CliArgs {
        backend,
        dump_dir,
        list_backends,
        dump_format,
        detection_samples_per_second,
        input,
    } = CliArgs::parse();

    if list_backends {
        display_available_backends();
        return Ok(());
    }

    let input = match input {
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
    let backend_override = match backend {
        Some(name) => Some(parse_backend(&name)?),
        None => None,
    };
    let backend_locked = backend_override.is_some() || env_backend_present;
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
                if !backend_locked {
                    if let Some(next_backend) = select_next_backend(&available, &tried) {
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

        match run_pipeline(
            provider,
            dump_dir.clone(),
            dump_format,
            detection_samples_per_second,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err((err, processed)) => {
                if processed == 0 && !backend_locked {
                    if let Some(next_backend) = select_next_backend(&available, &tried) {
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

fn parse_backend(value: &str) -> Result<Backend, YPlaneError> {
    Backend::from_str(value)
}

async fn run_pipeline(
    provider: DynYPlaneProvider,
    dump_dir: Option<PathBuf>,
    dump_format: DumpFormat,
    detection_samples_per_second: u32,
) -> Result<(), (YPlaneError, u64)> {
    let total_frames = provider.total_frames();
    let mut stream = provider.into_stream();

    let mut processed: u64 = 0;
    let sink_started = Instant::now();

    let (sink_bar, sink_progress_tx, sink_progress_task) =
        start_progress("processing", total_frames, sink_started);

    let sink_config = FrameSinkConfig::from_outputs(
        dump_dir,
        map_dump_format(dump_format),
        detection_samples_per_second,
    );

    let (frame_sink, mut sink_progress_rx) = FrameSink::new(sink_config);
    let sink_progress_sender = sink_progress_tx.clone();
    let progress_forward = tokio::spawn(async move {
        let mut completed = 0u64;
        while let Some(metadata) = sink_progress_rx.recv().await {
            completed = completed.saturating_add(1);
            let progress_event = ProgressEvent {
                index: completed,
                timestamp: metadata.timestamp,
            };
            if sink_progress_sender.send(progress_event).await.is_err() {
                break;
            }
        }
    });

    let mut failure: Option<YPlaneError> = None;

    while let Some(frame) = stream.next().await {
        match frame {
            Ok(frame) => {
                processed = processed.saturating_add(1);
                let timestamp = frame.timestamp();
                let frame_index = frame
                    .frame_index()
                    .unwrap_or_else(|| processed.saturating_sub(1));

                let metadata = FrameMetadata {
                    frame_index,
                    processed_index: processed,
                    timestamp,
                };
                if let Err(err) = frame_sink.submit(frame, metadata).await {
                    let reason = match err {
                        FrameSinkError::Stopped => "frame sink unavailable",
                    };
                    failure = Some(YPlaneError::configuration(reason));
                    break;
                }
            }
            Err(err) => {
                failure = Some(err);
                break;
            }
        }
    }

    if let Err(err) = frame_sink.shutdown().await {
        if failure.is_none() {
            failure = Some(YPlaneError::configuration(err.to_string()));
        }
    }

    drop(sink_progress_tx);
    let _ = progress_forward.await;
    let sink_summary = sink_progress_task.await.expect("progress task panicked");

    if let Some(err) = failure {
        sink_bar.abandon_with_message(format!(
            "failed after decoding {processed} frames; processed {} frames",
            sink_summary.processed
        ));
        return Err((err, processed));
    }

    finalize_success(&sink_bar, &sink_summary, total_frames);

    Ok(())
}

fn select_next_backend(available: &[Backend], tried: &[Backend]) -> Option<Backend> {
    available
        .iter()
        .copied()
        .find(|backend| !tried.contains(backend))
}

fn display_available_backends() {
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

fn map_dump_format(format: DumpFormat) -> ImageOutputFormat {
    match format {
        DumpFormat::Jpeg => ImageOutputFormat::Jpeg { quality: 90 },
        DumpFormat::Png => ImageOutputFormat::Png,
        DumpFormat::Webp => ImageOutputFormat::Webp,
        DumpFormat::Yuv => ImageOutputFormat::Yuv,
    }
}
