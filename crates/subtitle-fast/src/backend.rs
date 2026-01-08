use std::time::Instant;

use futures_util::StreamExt;
use subtitle_fast_decoder::{Backend, Configuration};
use subtitle_fast_types::DecoderError;

use crate::stage;

#[derive(Clone)]
pub struct ExecutionPlan {
    pub config: Configuration,
    pub backend_locked: bool,
    pub pipeline: stage::PipelineConfig,
}

pub async fn run(plan: ExecutionPlan) -> Result<(), DecoderError> {
    let ExecutionPlan {
        config,
        backend_locked,
        pipeline,
    } = plan;

    let available = Configuration::available_backends();
    if available.is_empty() {
        return Err(DecoderError::configuration(
            "no decoding backend available; rebuild with a backend feature such as \"backend-ffmpeg\"",
        ));
    }
    if !available.contains(&config.backend) {
        return Err(DecoderError::unsupported(config.backend.as_str()));
    }

    let mut attempt_config = config.clone();
    let mut tried = Vec::new();

    loop {
        if !tried.contains(&attempt_config.backend) {
            tried.push(attempt_config.backend);
        }

        let provider_started = Instant::now();
        let provider_result = attempt_config.create_provider();
        let provider_elapsed = provider_started.elapsed();

        let provider = match provider_result {
            Ok(provider) => {
                eprintln!(
                    "initialized decoder backend '{}' in {:.2?}",
                    attempt_config.backend.as_str(),
                    provider_elapsed
                );
                provider
            }
            Err(err) => {
                eprintln!(
                    "decoder backend '{}' failed to initialize in {:.2?}: {err}",
                    attempt_config.backend.as_str(),
                    provider_elapsed
                );
                if !backend_locked
                    && let Some(next_backend) = select_next_backend(&available, &tried)
                {
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
                return Err(err);
            }
        };

        let pipeline_result = stage::build_pipeline(provider, &pipeline);

        let outcome = match pipeline_result {
            Ok(pipeline_streams) => drive_pipeline(pipeline_streams, &pipeline.output.path).await,
            Err(err) => Err((err, 0)),
        };

        match outcome {
            Ok(()) => return Ok(()),
            Err((err, seen)) => {
                if seen == 0
                    && !backend_locked
                    && let Some(next_backend) = select_next_backend(&available, &tried)
                {
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
                return Err(err);
            }
        }
    }
}

pub fn display_available_backends() {
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

pub fn parse_backend(value: &str) -> Result<Backend, DecoderError> {
    use std::str::FromStr;
    Backend::from_str(value)
}

fn select_next_backend(available: &[Backend], tried: &[Backend]) -> Option<Backend> {
    available
        .iter()
        .copied()
        .find(|backend| !tried.contains(backend))
}

async fn drive_pipeline(
    pipeline: stage::PipelineOutputs,
    output_path: &std::path::Path,
) -> Result<(), (DecoderError, u64)> {
    let mut processed = 0;
    let mut subtitles: Vec<stage::MergedSubtitle> = Vec::new();
    let mut stream = pipeline.stream;

    while let Some(event) = stream.next().await {
        match event {
            Ok(update) => {
                processed = processed.max(update.progress.samples_seen);
                apply_updates(&mut subtitles, &update.updates);
            }
            Err(err) => {
                return Err((stage::pipeline_error_to_frame(err), processed));
            }
        }
    }

    sort_and_write(output_path, &subtitles)
        .await
        .map_err(|err| (err, processed))
}

fn apply_updates(subtitles: &mut Vec<stage::MergedSubtitle>, updates: &[stage::SubtitleUpdate]) {
    for update in updates {
        match update.kind {
            stage::SubtitleUpdateKind::New => {
                subtitles.push(update.subtitle.clone());
            }
            stage::SubtitleUpdateKind::Updated => {
                if let Some(existing) = subtitles
                    .iter_mut()
                    .find(|subtitle| subtitle.id == update.subtitle.id)
                {
                    *existing = update.subtitle.clone();
                } else {
                    subtitles.push(update.subtitle.clone());
                }
            }
        }
    }
}

async fn sort_and_write(
    output_path: &std::path::Path,
    subtitles: &[stage::MergedSubtitle],
) -> Result<(), DecoderError> {
    let mut ordered = subtitles.to_vec();
    stage::sort_subtitles(&mut ordered);
    let contents = stage::render_srt(&ordered);

    if let Some(parent) = output_path.parent().filter(|p| !p.as_os_str().is_empty())
        && let Err(err) = tokio::fs::create_dir_all(parent).await
    {
        return Err(DecoderError::configuration(format!(
            "failed to prepare subtitle directory {}: {err}",
            parent.display()
        )));
    }

    tokio::fs::write(output_path, contents)
        .await
        .map_err(|err| {
            DecoderError::configuration(format!(
                "failed to write subtitle file {}: {err}",
                output_path.display()
            ))
        })
}
