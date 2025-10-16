mod backend;
mod cli;
mod model;
mod pipeline;
mod progress;
mod settings;
mod stage;
mod tools;

use backend::ExecutionPlan;
use clap::CommandFactory;
use cli::{CliArgs, CliSources, OcrBackend, parse_cli};
use model::{ModelError, resolve_model_path};
use pipeline::PipelineConfig;
use settings::{ConfigError, resolve_settings};
use std::fs;
use std::num::NonZeroUsize;
use subtitle_fast_decoder::YPlaneError;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), YPlaneError> {
    match prepare_execution_plan().await? {
        Some(plan) => backend::run(plan).await,
        None => Ok(()),
    }
}

async fn prepare_execution_plan() -> Result<Option<ExecutionPlan>, YPlaneError> {
    let (cli_args, cli_sources): (CliArgs, CliSources) = parse_cli();

    if cli_args.list_backends {
        backend::display_available_backends();
        return Ok(None);
    }

    let input = match cli_args.input.clone() {
        Some(path) => path,
        None => {
            usage();
            return Ok(None);
        }
    };

    if !input.exists() {
        return Err(YPlaneError::configuration(format!(
            "input file '{}' does not exist",
            input.display()
        )));
    }

    let resolved = resolve_settings(&cli_args, &cli_sources).map_err(map_config_error)?;
    let config_dir = resolved.config_dir.clone();
    let settings = resolved.settings;

    if let Some(image) = settings.debug.image.as_ref() {
        fs::create_dir_all(&image.dir)?;
    }
    if let Some(json) = settings.debug.json.as_ref() {
        fs::create_dir_all(&json.dir)?;
    }

    #[cfg(all(feature = "ocr-mlx-vlm", target_os = "macos"))]
    let resolved_mlx_model_path = if matches!(settings.ocr.backend, OcrBackend::MlxVlm)
        || (matches!(settings.ocr.backend, OcrBackend::Auto)
            && settings.ocr.mlx_vlm_model.is_some())
    {
        resolve_model_path(
            settings.ocr.mlx_vlm_model.as_deref(),
            settings.ocr.mlx_vlm_model_from_cli,
            config_dir.as_deref(),
        )
        .await
        .map_err(map_model_error)?
    } else {
        None
    };

    #[cfg(not(all(feature = "ocr-mlx-vlm", target_os = "macos")))]
    let resolved_mlx_model_path: Option<std::path::PathBuf> = None;

    let pipeline = PipelineConfig::from_settings(&settings, resolved_mlx_model_path);

    let env_backend_present = std::env::var("SUBFAST_BACKEND").is_ok();
    let mut config = subtitle_fast_decoder::Configuration::from_env().unwrap_or_default();
    let backend_override = match settings.decoder.backend.as_ref() {
        Some(name) => Some(backend::parse_backend(name)?),
        None => None,
    };
    let backend_locked = backend_override.is_some() || env_backend_present;
    if let Some(backend_value) = backend_override {
        config.backend = backend_value;
    }
    config.input = Some(input);
    if let Some(capacity) = settings.decoder.channel_capacity {
        if let Some(non_zero) = NonZeroUsize::new(capacity) {
            config.channel_capacity = Some(non_zero);
        }
    }

    Ok(Some(ExecutionPlan {
        config,
        backend_locked,
        pipeline,
    }))
}

fn usage() {
    let mut command = CliArgs::command();
    command.print_help().ok();
    println!();
    backend::display_available_backends();
}

fn map_config_error(err: ConfigError) -> YPlaneError {
    YPlaneError::configuration(err.to_string())
}

fn map_model_error(err: ModelError) -> YPlaneError {
    YPlaneError::configuration(err.to_string())
}
