use std::path::PathBuf;

use clap::parser::ValueSource;
use clap::{ArgMatches, CommandFactory, FromArgMatches, Parser};

#[derive(Debug, Default)]
pub struct CliSources {
    pub detection_sps_from_cli: bool,
    pub decoder_channel_capacity_from_cli: bool,
    pub detector_target_from_cli: bool,
    pub detector_delta_from_cli: bool,
    pub comparator_from_cli: bool,
}

impl CliSources {
    fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            detection_sps_from_cli: value_from_cli(matches, "detection_samples_per_second"),
            decoder_channel_capacity_from_cli: value_from_cli(matches, "decoder_channel_capacity"),
            detector_target_from_cli: value_from_cli(matches, "detector_target"),
            detector_delta_from_cli: value_from_cli(matches, "detector_delta"),
            comparator_from_cli: value_from_cli(matches, "comparator"),
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

    /// Print the list of available decoding backends
    #[arg(long = "list-backends")]
    pub list_backends: bool,

    /// Subtitle detection samples per second
    #[arg(
        long = "detection-samples-per-second",
        alias = "detection-sps",
        default_value_t = 7,
        value_parser = parse_positive_u32
    )]
    pub detection_samples_per_second: u32,

    /// Decoder frame queue capacity before applying backpressure
    #[arg(
        long = "decoder-channel-capacity",
        id = "decoder_channel_capacity",
        value_parser = clap::value_parser!(usize)
    )]
    pub decoder_channel_capacity: Option<usize>,

    /// Override the detector target value (0-255)
    #[arg(long = "detector-target", value_parser = parse_u8_byte)]
    pub detector_target: Option<u8>,

    /// Override the detector delta value (0-255)
    #[arg(long = "detector-delta", value_parser = parse_u8_byte)]
    pub detector_delta: Option<u8>,

    /// Subtitle comparator to use (bitset-cover, sparse-chamfer)
    #[arg(long = "comparator")]
    pub comparator: Option<String>,

    /// Output subtitle file path
    #[arg(short = 'o', long = "output")]
    pub output: Option<PathBuf>,

    /// Input video path
    pub input: Option<PathBuf>,
}

fn parse_u8_byte(value: &str) -> Result<u8, String> {
    value
        .parse::<u8>()
        .map_err(|_| format!("'{value}' is not a valid 0-255 value"))
}

fn parse_positive_u32(value: &str) -> Result<u32, String> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| format!("'{value}' is not a valid number"))?;
    if parsed == 0 {
        return Err("value must be at least 1".into());
    }
    Ok(parsed)
}
