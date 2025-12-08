use std::path::PathBuf;

use clap::parser::ValueSource;
use clap::{ArgMatches, CommandFactory, FromArgMatches, Parser};
use subtitle_fast_types::RoiConfig;

#[derive(Debug, Default)]
pub struct CliSources {
    pub detection_sps_from_cli: bool,
    pub decoder_channel_capacity_from_cli: bool,
    pub detector_target_from_cli: bool,
    pub detector_delta_from_cli: bool,
    pub comparator_from_cli: bool,
    pub detector_roi_from_cli: bool,
}

impl CliSources {
    fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            detection_sps_from_cli: value_from_cli(matches, "detection_samples_per_second"),
            decoder_channel_capacity_from_cli: value_from_cli(matches, "decoder_channel_capacity"),
            detector_target_from_cli: value_from_cli(matches, "detector_target"),
            detector_delta_from_cli: value_from_cli(matches, "detector_delta"),
            comparator_from_cli: value_from_cli(matches, "comparator"),
            detector_roi_from_cli: value_from_cli(matches, "roi"),
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

    /// Normalized detection ROI as x,y,width,height (omit or zero size uses full frame)
    #[arg(long = "roi", value_name = "X,Y,W,H", value_parser = parse_roi)]
    pub roi: Option<RoiConfig>,

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

fn parse_roi(value: &str) -> Result<RoiConfig, String> {
    let parts: Vec<_> = value
        .split(|c| c == ',' || c == ' ')
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() != 4 {
        return Err("roi must be four numbers: x,y,width,height".into());
    }
    let parse = |s: &str| {
        s.parse::<f32>()
            .map_err(|_| format!("'{s}' is not a valid number"))
    };
    let x = parse(parts[0])?;
    let y = parse(parts[1])?;
    let width = parse(parts[2])?;
    let height = parse(parts[3])?;
    if x < 0.0 || y < 0.0 {
        return Err("roi coordinates must be non-negative".into());
    }
    if width < 0.0 || height < 0.0 {
        return Err("roi width and height must be non-negative".into());
    }
    Ok(RoiConfig {
        x,
        y,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_roi_accepts_commas() {
        let roi = parse_roi("0.1,0.2,0.3,0.4").unwrap();
        assert_eq!(
            roi,
            RoiConfig {
                x: 0.1,
                y: 0.2,
                width: 0.3,
                height: 0.4
            }
        );
    }

    #[test]
    fn parse_roi_accepts_spaces() {
        let roi = parse_roi("0.1 0.2 0.5 0.6").unwrap();
        assert_eq!(
            roi,
            RoiConfig {
                x: 0.1,
                y: 0.2,
                width: 0.5,
                height: 0.6
            }
        );
    }

    #[test]
    fn parse_roi_rejects_invalid_counts() {
        assert!(parse_roi("0.1,0.2,0.3").is_err());
        assert!(parse_roi("0.1").is_err());
    }

    #[test]
    fn parse_roi_rejects_negative_values() {
        assert!(parse_roi("-0.1,0.0,0.5,0.5").is_err());
        assert!(parse_roi("0.0,0.0,-0.5,0.5").is_err());
    }
}
