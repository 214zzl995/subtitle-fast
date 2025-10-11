use std::time::Instant;

use subtitle_fast_decoder::{Backend, Configuration, YPlaneError};

use crate::pipeline;

#[derive(Clone)]
pub struct ExecutionPlan {
    pub config: Configuration,
    pub backend_locked: bool,
    pub pipeline: pipeline::PipelineConfig,
}

pub async fn run(plan: ExecutionPlan) -> Result<(), YPlaneError> {
    let ExecutionPlan {
        config,
        backend_locked,
        pipeline,
    } = plan;

    let available = Configuration::available_backends();
    if available.is_empty() {
        return Err(YPlaneError::configuration(
            "no decoding backend available; rebuild with a backend feature such as \"backend-ffmpeg\"",
        ));
    }
    if !available.contains(&config.backend) {
        return Err(YPlaneError::unsupported(config.backend.as_str()));
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

        match pipeline::run_pipeline(provider, &pipeline).await {
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

pub fn parse_backend(value: &str) -> Result<Backend, YPlaneError> {
    use std::str::FromStr;
    Backend::from_str(value)
}

fn select_next_backend(available: &[Backend], tried: &[Backend]) -> Option<Backend> {
    available
        .iter()
        .copied()
        .find(|backend| !tried.contains(backend))
}
