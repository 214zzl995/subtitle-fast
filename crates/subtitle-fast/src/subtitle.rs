use std::fmt::Write as _;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct SubtitleLine {
    pub center: f32,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct MergedSubtitle {
    pub id: u64,
    pub start_time: Duration,
    pub end_time: Duration,
    pub start_frame: u64,
    pub lines: Vec<SubtitleLine>,
}

#[derive(Clone, Debug)]
pub struct TimedSubtitle {
    pub id: u64,
    pub start_ms: f64,
    pub end_ms: f64,
    pub lines: Vec<String>,
}

impl MergedSubtitle {
    pub fn as_timed(&self) -> TimedSubtitle {
        TimedSubtitle {
            id: self.id,
            start_ms: self.start_time.as_secs_f64() * 1000.0,
            end_ms: self.end_time.as_secs_f64() * 1000.0,
            lines: ordered_lines(&self.lines),
        }
    }
}

impl TimedSubtitle {
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }
}

pub fn sort_subtitles(subtitles: &mut [MergedSubtitle]) {
    subtitles.sort_by(|a, b| match a.start_time.cmp(&b.start_time) {
        std::cmp::Ordering::Equal => a.start_frame.cmp(&b.start_frame),
        other => other,
    });
}

pub fn render_srt(subtitles: &[MergedSubtitle]) -> String {
    let mut output = String::new();
    for (idx, cue) in subtitles.iter().enumerate() {
        let lines = ordered_lines(&cue.lines);
        if lines.is_empty() {
            continue;
        }
        if idx > 0 {
            output.push('\n');
        }
        let _ = writeln!(&mut output, "{}", idx + 1);
        let _ = writeln!(
            &mut output,
            "{} --> {}",
            format_timestamp(cue.start_time),
            format_timestamp(cue.end_time)
        );
        for line in lines {
            let _ = writeln!(&mut output, "{line}");
        }
    }
    output
}

fn ordered_lines(lines: &[SubtitleLine]) -> Vec<String> {
    let mut refs: Vec<&SubtitleLine> = lines.iter().collect();
    refs.sort_by(|a, b| {
        a.center
            .partial_cmp(&b.center)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut ordered = Vec::new();
    for line in refs {
        let text = line.text.trim();
        if text.is_empty() {
            continue;
        }
        if ordered.last().is_some_and(|last: &String| last == text) {
            continue;
        }
        ordered.push(text.to_string());
    }
    ordered
}

fn format_timestamp(time: Duration) -> String {
    let millis = time
        .as_secs()
        .saturating_mul(1000)
        .saturating_add(u64::from(time.subsec_millis()));
    let hours = millis / 3_600_000;
    let minutes = (millis % 3_600_000) / 60_000;
    let seconds = (millis % 60_000) / 1000;
    let remain_ms = millis % 1000;
    format!("{hours:02}:{minutes:02}:{seconds:02},{remain_ms:03}")
}
