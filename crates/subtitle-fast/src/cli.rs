use std::path::PathBuf;

use clap::parser::ValueSource;
use clap::{ArgMatches, CommandFactory, FromArgMatches, Parser, ValueEnum};

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum DumpFormat {
    Jpeg,
    Png,
    Webp,
    Yuv,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum DetectionBackend {
    Auto,
    Onnx,
    Vision,
}

#[derive(Debug, Default)]
pub struct CliSources {
    pub dump_format_from_cli: bool,
    pub detection_backend_from_cli: bool,
    pub detection_sps_from_cli: bool,
    pub onnx_model_from_cli: bool,
}

impl CliSources {
    fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            dump_format_from_cli: value_from_cli(matches, "dump_format"),
            detection_backend_from_cli: value_from_cli(matches, "detection_backend"),
            detection_sps_from_cli: value_from_cli(matches, "detection_samples_per_second"),
            onnx_model_from_cli: value_from_cli(matches, "onnx_model"),
        }
    }
}

fn value_from_cli(matches: &ArgMatches, id: &str) -> bool {
    matches
        .value_source(id)
        .is_some_and(|source| matches!(source, ValueSource::CommandLine))
}

pub fn parse_cli() -> (CliArgs, CliSources) {
    let command = CliArgs::command();
    let matches = command.get_matches();
    let args = match CliArgs::from_arg_matches(&matches) {
        Ok(args) => args,
        Err(err) => err.exit(),
    };
    let sources = CliSources::from_matches(&matches);
    (args, sources)
}

#[derive(Debug, Parser)]
#[command(
    name = "subtitle-fast",
    about = "Decode video frames and detect subtitles",
    disable_help_subcommand = true
)]
pub struct CliArgs {
    /// Lock decoding to a specific backend implementation
    #[arg(short = 'b', long = "backend")]
    pub backend: Option<String>,

    /// Override the configuration file path
    #[arg(long = "config")]
    pub config: Option<PathBuf>,

    /// Output directory for writing sampled frames as image files
    #[arg(long = "dump-dir")]
    pub dump_dir: Option<PathBuf>,

    /// Print the list of available decoding backends
    #[arg(long = "list-backends")]
    pub list_backends: bool,

    /// Image format for dumped frames when --dump-dir is set
    #[arg(long = "dump-format", value_enum, default_value_t = DumpFormat::Jpeg)]
    pub dump_format: DumpFormat,

    /// Subtitle detection samples per second
    #[arg(
        long = "detection-samples-per-second",
        alias = "detection-sps",
        default_value_t = 7,
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    pub detection_samples_per_second: u32,

    /// Preferred subtitle detection backend
    #[arg(long = "detection-backend", value_enum)]
    #[cfg_attr(target_os = "macos", arg(default_value_t = DetectionBackend::Vision))]
    #[cfg_attr(not(target_os = "macos"), arg(default_value_t = DetectionBackend::Onnx))]
    pub detection_backend: DetectionBackend,

    /// Path or URI to the ONNX subtitle detection model
    #[arg(long = "onnx-model")]
    pub onnx_model: Option<String>,

    /// Input video path
    pub input: Option<PathBuf>,
}
