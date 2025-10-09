use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Instant;

mod cli;
mod model;
mod progress;
mod settings;

use clap::CommandFactory;
use cli::{CliArgs, DetectionBackend, DumpFormat, parse_cli};
use model::{ModelError, resolve_model_path};
use progress::{ProgressEvent, finalize_success, start_progress};
use settings::{ConfigError, resolve_settings};
use subtitle_fast_decoder::{Backend, Configuration, DynYPlaneProvider, YPlaneError};
use subtitle_fast_sink::subtitle_detection::{SubtitleDetectionError, preflight_detection};
use subtitle_fast_sink::{
    FrameMetadata, FrameSink, FrameSinkConfig, FrameSinkError, ImageOutputFormat,
    SubtitleDetectorKind,
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
    let (cli_args, cli_sources) = parse_cli();

    if cli_args.list_backends {
        display_available_backends();
        return Ok(());
    }

    let input = match cli_args.input.clone() {
        Some(path) => path,
        None => {
            usage();
            return Ok(());
        }
    };

    let settings = resolve_settings(&cli_args, &cli_sources).map_err(map_config_error)?;

    if let Some(dir) = settings.dump_dir.as_ref() {
        fs::create_dir_all(dir)?;
    }

    let resolved_model_path = resolve_model_path(
        settings.onnx_model.as_deref(),
        settings.onnx_model_from_cli,
        settings.config_dir.as_deref(),
    )
    .await
    .map_err(map_model_error)?;

    let detection_kind = map_detection_backend(settings.detection_backend);

    let env_backend_present = std::env::var("SUBFAST_BACKEND").is_ok();
    let mut config = Configuration::from_env().unwrap_or_default();

    let backend_override = match settings.backend.as_ref() {
        Some(name) => Some(parse_backend(name)?),
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
            settings.dump_dir.clone(),
            settings.dump_format,
            settings.detection_samples_per_second,
            detection_kind,
            resolved_model_path.clone(),
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
    detection_backend: SubtitleDetectorKind,
    onnx_model_path: Option<PathBuf>,
) -> Result<(), (YPlaneError, u64)> {
    let total_frames = provider.total_frames();
    let mut stream = provider.into_stream();

    let mut sink_config = FrameSinkConfig::from_outputs(
        dump_dir,
        map_dump_format(dump_format),
        detection_samples_per_second,
    );
    sink_config.detection.detector = detection_backend;
    sink_config.detection.onnx_model_path = onnx_model_path;

    if sink_config.detection.enabled {
        let model_path = sink_config.detection.onnx_model_path.as_deref();
        if let Err(err) = preflight_detection(sink_config.detection.detector, model_path) {
            panic_with_detection_error(err);
        }
    }

    let mut processed: u64 = 0;
    let sink_started = Instant::now();

    let (sink_bar, sink_progress_tx, sink_progress_task) =
        start_progress("processing", total_frames, sink_started);

    let (frame_sink, mut sink_progress_rx) = match FrameSink::new(sink_config) {
        Ok(result) => result,
        Err(err) => panic_with_detection_error(err),
    };
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

fn map_detection_backend(backend: DetectionBackend) -> SubtitleDetectorKind {
    match backend {
        DetectionBackend::Auto => SubtitleDetectorKind::Auto,
        DetectionBackend::Onnx => SubtitleDetectorKind::OnnxPpocr,
        DetectionBackend::Vision => SubtitleDetectorKind::MacVision,
    }
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

fn panic_with_detection_error(err: SubtitleDetectionError) -> ! {
    match err {
        SubtitleDetectionError::Environment(_)
        | SubtitleDetectionError::Session(_)
        | SubtitleDetectionError::ModelNotFound { .. }
        | SubtitleDetectionError::Input(_)
        | SubtitleDetectionError::Inference(_)
        | SubtitleDetectionError::InvalidOutputShape
        | SubtitleDetectionError::RuntimeSchemaConflict { .. } => panic!(
            "failed to initialize the ONNX subtitle detector: {err}\n\
             Install the ONNX Runtime 1.16.x shared libraries and ensure the dynamic library \
             directory is visible to the application (for example via ORT_DYLIB_PATH or your \
             system's library path). Documentation: https://onnxruntime.ai/docs/install/"
        ),
        _ => panic!("failed to initialize the subtitle detector: {err}"),
    }
}

fn map_config_error(err: ConfigError) -> YPlaneError {
    YPlaneError::configuration(err.to_string())
}

fn map_model_error(err: ModelError) -> YPlaneError {
    YPlaneError::configuration(err.to_string())
}
