use std::env;
use std::num::NonZeroUsize;

use clap::CommandFactory;
use subtitle_fast::backend::{self, ExecutionPlan};
use subtitle_fast::cli::{CliArgs, CliSources, parse_cli};
use subtitle_fast::settings::{ConfigError, resolve_settings};
use subtitle_fast::stage::PipelineConfig;
use subtitle_fast_types::DecoderError;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), DecoderError> {
    #[allow(unused_variables)]
    let args: Vec<String> = env::args().collect();

    #[cfg(feature = "gui")]
    {
        if args.len() == 1 {
            return run_gui();
        }
    }

    run_cli().await
}

#[cfg(feature = "gui")]
fn run_gui() -> Result<(), DecoderError> {
    use gpui::*;
    use subtitle_fast::gui::{AppAssets, SubtitleFastApp};

    Application::new()
        .with_assets(AppAssets)
        .run(|cx: &mut App| {
            gpui_component::init(cx);
            let app = SubtitleFastApp::new(cx);
            app.open_window(cx);
            cx.activate(true);
        });

    Ok(())
}

async fn run_cli() -> Result<(), DecoderError> {
    match prepare_execution_plan().await? {
        Some(plan) => backend::run(plan).await,
        None => Ok(()),
    }
}

async fn prepare_execution_plan() -> Result<Option<ExecutionPlan>, DecoderError> {
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
        return Err(DecoderError::configuration(format!(
            "input file '{}' does not exist",
            input.display()
        )));
    }

    let resolved = resolve_settings(&cli_args, &cli_sources).map_err(map_config_error)?;
    let settings = resolved.settings;

    let pipeline = PipelineConfig::from_settings(&settings, &input)?;

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
    if let Some(capacity) = settings.decoder.channel_capacity
        && let Some(non_zero) = NonZeroUsize::new(capacity)
    {
        config.channel_capacity = Some(non_zero);
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

fn map_config_error(err: ConfigError) -> DecoderError {
    DecoderError::configuration(err.to_string())
}
