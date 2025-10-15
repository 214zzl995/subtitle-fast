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
    Luma,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum OcrBackend {
    Auto,
    Vision,
    Onnx,
    Noop,
    MlxVlm,
}

#[derive(Debug, Default)]
pub struct CliSources {
    pub dump_format_from_cli: bool,
    pub detection_backend_from_cli: bool,
    pub detection_sps_from_cli: bool,
    pub detection_onnx_model_from_cli: bool,
    pub ocr_backend_from_cli: bool,
    pub ocr_onnx_model_from_cli: bool,
    pub ocr_mlx_model_from_cli: bool,
    pub ocr_languages_from_cli: bool,
    pub ocr_auto_detect_language_from_cli: bool,
    pub detection_luma_target_from_cli: bool,
    pub detection_luma_delta_from_cli: bool,
    pub decoder_channel_capacity_from_cli: bool,
}

impl CliSources {
    fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            dump_format_from_cli: value_from_cli(matches, "dump_format"),
            detection_backend_from_cli: value_from_cli(matches, "detection_backend"),
            detection_sps_from_cli: value_from_cli(matches, "detection_samples_per_second"),
            detection_onnx_model_from_cli: value_from_cli(matches, "onnx_model"),
            ocr_backend_from_cli: value_from_cli(matches, "ocr_backend"),
            ocr_onnx_model_from_cli: value_from_cli(matches, "ocr_onnx_model"),
            ocr_mlx_model_from_cli: value_from_cli(matches, "ocr_mlx_model"),
            ocr_languages_from_cli: value_from_cli(matches, "ocr_languages"),
            ocr_auto_detect_language_from_cli: value_from_cli(matches, "ocr_auto_detect_language"),
            detection_luma_target_from_cli: value_from_cli(matches, "detection_luma_target"),
            detection_luma_delta_from_cli: value_from_cli(matches, "detection_luma_delta"),
            decoder_channel_capacity_from_cli: value_from_cli(matches, "decoder_channel_capacity"),
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

    /// Final output path for generated subtitle data
    #[arg(long = "output")]
    pub output: Option<PathBuf>,

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
    #[arg(
        long = "detection-backend",
        value_enum,
        default_value_t = DetectionBackend::Luma
    )]
    pub detection_backend: DetectionBackend,

    /// Path or URI to the ONNX subtitle detection model
    #[arg(long = "onnx-model")]
    pub onnx_model: Option<String>,

    /// Preferred OCR backend
    #[arg(long = "ocr-backend", value_enum, default_value_t = OcrBackend::Auto)]
    pub ocr_backend: OcrBackend,

    /// Path or URI to the ONNX OCR model
    #[arg(long = "ocr-onnx-model", id = "ocr_onnx_model")]
    pub ocr_onnx_model: Option<String>,

    /// Local model path for the mlx_vlm OCR backend
    #[arg(long = "ocr-mlx-model", id = "ocr_mlx_model")]
    pub ocr_mlx_model: Option<String>,

    /// Restrict OCR to the provided language (repeatable)
    #[arg(long = "ocr-language", id = "ocr_languages", value_name = "LANG")]
    pub ocr_languages: Vec<String>,

    /// Enable or disable automatic language detection inside the OCR backend
    #[arg(
        long = "ocr-auto-detect-language",
        id = "ocr_auto_detect_language",
        value_parser = clap::value_parser!(bool)
    )]
    pub ocr_auto_detect_language: Option<bool>,

    /// Target Y-plane brightness used by the luma-band detector (0-255)
    #[arg(
        long = "detection-luma-target",
        id = "detection_luma_target",
        value_parser = clap::value_parser!(u8)
    )]
    pub detection_luma_target: Option<u8>,

    /// Allowed deviation around the target brightness for the luma-band detector (0-255)
    #[arg(
        long = "detection-luma-delta",
        id = "detection_luma_delta",
        value_parser = clap::value_parser!(u8)
    )]
    pub detection_luma_delta: Option<u8>,

    /// Decoder frame queue capacity before applying backpressure
    #[arg(
        long = "decoder-channel-capacity",
        id = "decoder_channel_capacity",
        value_parser = clap::value_parser!(usize)
    )]
    pub decoder_channel_capacity: Option<usize>,

    /// Input video path
    pub input: Option<PathBuf>,
}
