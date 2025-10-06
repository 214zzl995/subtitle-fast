use std::path::PathBuf;

use clap::Parser;

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

    /// Output directory for writing decoded frames as JPEG files
    #[arg(long = "dump-dir")]
    pub dump_dir: Option<PathBuf>,

    /// Print the list of available decoding backends
    #[arg(long = "list-backends")]
    pub list_backends: bool,

    /// Subtitle detection samples per second
    #[arg(
        long = "detection-samples-per-second",
        alias = "detection-sps",
        default_value_t = 7,
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    pub detection_samples_per_second: u32,

    /// Input video path
    pub input: Option<PathBuf>,
}
