#![allow(dead_code)]

use indicatif::ProgressStyle;

pub fn sampling_bar_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{prefix:<10} {bar:40.cyan/blue} {percent:>3}% {pos}/{len} frames [{elapsed_precise}<{eta_precise}] speed {msg}",
    )
    .expect("invalid sampling bar template")
}

pub fn sampling_spinner_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{prefix:<10} {spinner:.cyan.bold} [{elapsed_precise}] frames {pos} • speed {msg}",
    )
    .expect("invalid sampling spinner template")
    .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
}
