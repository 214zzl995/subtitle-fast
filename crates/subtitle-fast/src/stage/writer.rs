use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream::unfold};
use tokio::fs;
use tokio::sync::mpsc;

use super::StreamBundle;
use super::detector::DetectionSample;
use super::ocr::{OcrEvent, OcrStageError, OcrStageResult, OcrTimings};
use super::segmenter::SegmentTimings;
use subtitle_fast_ocr::OcrResponse;

const WRITER_CHANNEL_CAPACITY: usize = 4;

pub type WriterResult = Result<WriterEvent, SubtitleWriterError>;

pub struct SubtitleWriter {
    output_path: PathBuf,
}

impl SubtitleWriter {
    pub fn new(output_path: PathBuf) -> Self {
        Self { output_path }
    }

    pub fn attach(self, input: StreamBundle<OcrStageResult>) -> StreamBundle<WriterResult> {
        let StreamBundle {
            stream,
            total_frames,
        } = input;

        let (tx, rx) = mpsc::channel::<WriterResult>(WRITER_CHANNEL_CAPACITY);
        let output_path = self.output_path;

        tokio::spawn(async move {
            let mut upstream = stream;
            let mut worker = SubtitleWriterWorker::new(output_path);

            while let Some(event) = upstream.next().await {
                match event {
                    Ok(ocr_event) => {
                        let writer_event = worker.handle_event(ocr_event);
                        if tx.send(Ok(writer_event)).await.is_err() {
                            return;
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(Err(SubtitleWriterError::Ocr(err))).await;
                        return;
                    }
                }
            }

            match worker.finish().await {
                Ok(final_event) => {
                    let _ = tx.send(Ok(final_event)).await;
                }
                Err(err) => {
                    let _ = tx.send(Err(err)).await;
                }
            }
        });

        let stream = Box::pin(unfold(rx, |mut receiver| async {
            receiver.recv().await.map(|item| (item, receiver))
        }));

        StreamBundle::new(stream, total_frames)
    }
}

pub struct WriterEvent {
    pub sample: Option<DetectionSample>,
    pub segment_timings: Option<SegmentTimings>,
    pub ocr_timings: Option<OcrTimings>,
    pub writer_timings: Option<WriterTimings>,
    pub status: WriterStatus,
    pub last_subtitle: Option<GuiSubtitleInfo>,
}

#[derive(Clone)]
pub struct GuiSubtitleInfo {
    pub start_ms: f64,
    pub end_ms: f64,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WriterTimings {
    pub cues: u64,
    pub ocr_empty: u64,
    pub total: Duration,
}

#[derive(Debug, Clone)]
pub enum WriterStatus {
    Pending,
    Completed { path: PathBuf, cues: usize },
}

#[derive(Debug)]
pub enum SubtitleWriterError {
    Ocr(OcrStageError),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

#[derive(Clone)]
struct SubtitleCue {
    start_time: Duration,
    end_time: Duration,
    start_frame: u64,
    text: String,
    center: f32,
}

struct SubtitleWriterWorker {
    output_path: PathBuf,
    cues: Vec<SubtitleCue>,
}

impl SubtitleWriterWorker {
    fn new(output_path: PathBuf) -> Self {
        Self {
            output_path,
            cues: Vec::new(),
        }
    }

    fn handle_event(&mut self, event: OcrEvent) -> WriterEvent {
        let started = Instant::now();
        let mut buffered = 0_u64;
        let mut ocr_empty = 0_u64;
        let mut last_subtitle: Option<GuiSubtitleInfo> = None;

        for subtitle in event.subtitles {
            let text = response_to_text(&subtitle.response);
            if text.is_empty() {
                ocr_empty = ocr_empty.saturating_add(1);
                continue;
            }
            let center = subtitle.region.y + subtitle.region.height * 0.5;
            let cue = SubtitleCue {
                start_time: subtitle.interval.start_time,
                end_time: subtitle.interval.end_time,
                start_frame: subtitle.interval.start_frame,
                text,
                center,
            };
            self.cues.push(cue.clone());
            buffered = buffered.saturating_add(1);
            last_subtitle = Some(GuiSubtitleInfo {
                start_ms: subtitle.interval.start_time.as_secs_f64() * 1000.0,
                end_ms: subtitle.interval.end_time.as_secs_f64() * 1000.0,
                text: cue.text.clone(),
            });
        }

        let timings = WriterTimings {
            cues: buffered,
            ocr_empty,
            total: started.elapsed(),
        };

        WriterEvent {
            sample: event.sample,
            segment_timings: event.segment_timings,
            ocr_timings: event.timings,
            writer_timings: Some(timings),
            status: WriterStatus::Pending,
            last_subtitle,
        }
    }

    async fn finish(self) -> Result<WriterEvent, SubtitleWriterError> {
        let SubtitleWriterWorker {
            output_path,
            mut cues,
        } = self;

        sort_cues(&mut cues);
        let merged = merge_cues(&cues);
        let started = Instant::now();
        let srt_contents = build_srt(&merged);

        if let Some(parent) = output_path.parent().filter(|p| !p.as_os_str().is_empty())
            && let Err(err) = fs::create_dir_all(parent).await
        {
            return Err(SubtitleWriterError::Io {
                path: output_path.clone(),
                source: err,
            });
        }

        if let Err(err) = fs::write(&output_path, srt_contents).await {
            return Err(SubtitleWriterError::Io {
                path: output_path.clone(),
                source: err,
            });
        }

        let elapsed = started.elapsed();
        let cue_count = merged.len();
        let timings = WriterTimings {
            cues: cue_count as u64,
            ocr_empty: 0,
            total: elapsed,
        };

        Ok(WriterEvent {
            sample: None,
            segment_timings: None,
            ocr_timings: None,
            writer_timings: Some(timings),
            status: WriterStatus::Completed {
                path: output_path,
                cues: cue_count,
            },
            last_subtitle: None,
        })
    }
}

fn response_to_text(response: &OcrResponse) -> String {
    if response.texts.is_empty() {
        return String::new();
    }
    let mut parts = Vec::new();
    for entry in &response.texts {
        let trimmed = entry.text.trim();
        if trimmed.is_empty() {
            continue;
        }
        parts.push(trimmed.to_string());
    }
    parts.join("\n")
}

fn sort_cues(cues: &mut [SubtitleCue]) {
    cues.sort_by(|a, b| match a.start_time.cmp(&b.start_time) {
        std::cmp::Ordering::Equal => a.start_frame.cmp(&b.start_frame),
        other => other,
    });
}

fn build_srt(cues: &[MergedSubtitle]) -> String {
    let mut output = String::new();
    for (idx, cue) in cues.iter().enumerate() {
        let lines = ordered_lines(cue);
        if lines.is_empty() {
            continue;
        }
        if idx > 0 {
            output.push('\n');
        }
        writeln!(&mut output, "{}", idx + 1).expect("write to string");
        writeln!(
            &mut output,
            "{} --> {}",
            format_timestamp(cue.start_time),
            format_timestamp(cue.end_time)
        )
        .expect("write to string");
        for line in lines {
            writeln!(&mut output, "{line}").expect("write to string");
        }
    }
    output
}

fn ordered_lines(cue: &MergedSubtitle) -> Vec<String> {
    let mut refs: Vec<&SubtitleLine> = cue.lines.iter().collect();
    refs.sort_by(|a, b| {
        a.center
            .partial_cmp(&b.center)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut lines = Vec::new();
    for line in refs {
        let text = line.text.trim();
        if text.is_empty() {
            continue;
        }
        if lines.last().is_some_and(|last: &String| last == text) {
            continue;
        }
        lines.push(text.to_string());
    }
    lines
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

#[derive(Clone)]
struct SubtitleLine {
    center: f32,
    text: String,
}

struct MergedSubtitle {
    start_time: Duration,
    end_time: Duration,
    lines: Vec<SubtitleLine>,
}

fn merge_cues(cues: &[SubtitleCue]) -> Vec<MergedSubtitle> {
    let mut merged = Vec::new();
    for cue in cues {
        if let Some(last) = merged.last_mut()
            && should_merge(last, cue)
        {
            last.start_time = last.start_time.min(cue.start_time);
            last.end_time = last.end_time.max(cue.end_time);
            if !last.lines.iter().any(|line| line.text == cue.text) {
                last.lines.push(SubtitleLine {
                    center: cue.center,
                    text: cue.text.clone(),
                });
            }
            continue;
        }
        merged.push(MergedSubtitle {
            start_time: cue.start_time,
            end_time: cue.end_time,
            lines: vec![SubtitleLine {
                center: cue.center,
                text: cue.text.clone(),
            }],
        });
    }
    merged
}

const MERGE_GAP: Duration = Duration::from_millis(120);

fn should_merge(current: &MergedSubtitle, incoming: &SubtitleCue) -> bool {
    if incoming.start_time <= current.end_time {
        return true;
    }
    let gap = incoming
        .start_time
        .checked_sub(current.end_time)
        .unwrap_or(Duration::ZERO);
    if gap <= MERGE_GAP {
        current.lines.iter().any(|line| line.text == incoming.text)
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SubtitleCue, build_srt, format_timestamp, merge_cues, ordered_lines, response_to_text,
        sort_cues,
    };
    use std::time::Duration;
    use subtitle_fast_ocr::{OcrRegion, OcrResponse, OcrText};

    #[test]
    fn timestamp_formatting_matches_srt() {
        let ts = Duration::from_millis(3_723_456);
        assert_eq!(format_timestamp(ts), "01:02:03,456");
    }

    #[test]
    fn srt_builder_preserves_order() {
        let mut cues = vec![
            SubtitleCue {
                start_time: Duration::from_secs(5),
                end_time: Duration::from_secs(7),
                start_frame: 10,
                text: "Second".into(),
                center: 0.6,
            },
            SubtitleCue {
                start_time: Duration::from_secs(2),
                end_time: Duration::from_secs(3),
                start_frame: 5,
                text: "First".into(),
                center: 0.4,
            },
        ];
        sort_cues(&mut cues);
        let merged = merge_cues(&cues);
        let output = build_srt(&merged);
        assert!(output.starts_with("1\n00:00:02,000 --> 00:00:03,000\nFirst"));
        assert!(output.contains("2\n00:00:05,000 --> 00:00:07,000\nSecond"));
    }

    #[test]
    fn response_to_text_drops_empty_entries() {
        let response = OcrResponse::new(vec![
            OcrText::new(OcrRegion::new(0.0, 0.0, 1.0, 1.0), " hello ".into()),
            OcrText::new(OcrRegion::new(0.0, 0.0, 1.0, 1.0), "   ".into()),
            OcrText::new(OcrRegion::new(0.0, 0.0, 1.0, 1.0), "world".into()),
        ]);
        assert_eq!(response_to_text(&response), "hello\nworld");
    }

    #[test]
    fn merge_cues_combines_overlapping_lines() {
        let mut cues = vec![
            SubtitleCue {
                start_time: Duration::from_secs(0),
                end_time: Duration::from_secs(2),
                start_frame: 1,
                text: "Line A".into(),
                center: 0.9,
            },
            SubtitleCue {
                start_time: Duration::from_millis(100),
                end_time: Duration::from_secs(2),
                start_frame: 2,
                text: "Line B".into(),
                center: 0.8,
            },
        ];
        sort_cues(&mut cues);
        let merged = merge_cues(&cues);
        assert_eq!(merged.len(), 1);
        let lines = ordered_lines(&merged[0]);
        assert_eq!(lines, vec!["Line B".to_string(), "Line A".to_string()]);
    }
}
