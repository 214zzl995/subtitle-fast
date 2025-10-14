use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::{Mutex, mpsc};

use super::{PipelineStage, StageInput, StageOutput};
use crate::stage::detection::{SubtitleSegment, SubtitleStageError, SubtitleStageResult};
use subtitle_fast_ocr::{LumaPlane, OcrEngine, OcrError, OcrRegion, OcrRequest, OcrResponse};
use subtitle_fast_validator::subtitle_detection::RoiConfig;

const LINE_MERGE_THRESHOLD_PX: f32 = 12.0;

pub type SubtitleGenResult = Result<GeneratedSubtitle, SubtitleGenError>;

/// 生成字幕: converts detected subtitle segments into final text output.
pub struct SubtitleGen<E>
where
    E: OcrEngine + 'static + ?Sized,
{
    engine: Arc<E>,
    writer: Option<Arc<SubtitleWriter>>,
}

impl<E> SubtitleGen<E>
where
    E: OcrEngine + 'static + ?Sized,
{
    pub fn new(engine: Arc<E>, output_path: Option<PathBuf>) -> Self {
        let writer = output_path.map(|path| Arc::new(SubtitleWriter::new(path)));
        Self { engine, writer }
    }
}

impl<E> PipelineStage<SubtitleStageResult> for SubtitleGen<E>
where
    E: OcrEngine + 'static + ?Sized,
{
    type Output = SubtitleGenResult;

    fn name(&self) -> &'static str {
        "subtitle_gen"
    }

    fn apply(self: Box<Self>, input: StageInput<SubtitleStageResult>) -> StageOutput<Self::Output> {
        let StageInput {
            stream,
            total_frames,
        } = input;

        let engine = self.engine.clone();
        let writer = self.writer.clone();
        let (tx, rx) = mpsc::channel::<SubtitleGenResult>(24);

        tokio::spawn(async move {
            let mut upstream = stream;
            let mut worker = SubtitleGenWorker::new(engine, writer);

            while let Some(item) = upstream.next().await {
                match item {
                    Ok(segment) => match worker.handle_segment(segment).await {
                        Ok(Some(result)) => {
                            if tx.send(Ok(result)).await.is_err() {
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(err) => {
                            let _ = tx.send(Err(err)).await;
                            break;
                        }
                    },
                    Err(err) => {
                        let _ = tx.send(Err(SubtitleGenError::Upstream(err))).await;
                        break;
                    }
                }
            }

            if let Some(writer) = worker.writer.clone() {
                if let Err(err) = writer.finalize().await {
                    let _ = tx.send(Err(SubtitleGenError::Writer(err))).await;
                }
            }
        });

        let stream = Box::pin(futures_util::stream::unfold(rx, |mut receiver| async {
            match receiver.recv().await {
                Some(item) => Some((item, receiver)),
                None => None,
            }
        }));

        StageOutput {
            stream,
            total_frames,
        }
    }
}

pub struct GeneratedSubtitle {
    pub start: Duration,
    pub end: Duration,
    pub text: String,
    pub confidence: Option<f32>,
    pub frame_index: Option<u64>,
}

pub enum SubtitleGenError {
    Upstream(SubtitleStageError),
    Ocr(OcrError),
    Writer(io::Error),
    Join(String),
}

struct SubtitleGenWorker<E>
where
    E: OcrEngine + 'static + ?Sized,
{
    engine: Arc<E>,
    writer: Option<Arc<SubtitleWriter>>,
}

impl<E> SubtitleGenWorker<E>
where
    E: OcrEngine + 'static + ?Sized,
{
    fn new(engine: Arc<E>, writer: Option<Arc<SubtitleWriter>>) -> Self {
        Self { engine, writer }
    }

    async fn handle_segment(
        &mut self,
        segment: SubtitleSegment,
    ) -> Result<Option<GeneratedSubtitle>, SubtitleGenError> {
        let start = segment.start.or_else(|| segment.frame.timestamp());
        let end = segment.end.or_else(|| segment.frame.timestamp());

        let (Some(start), Some(mut end)) = (start, end) else {
            eprintln!("skipping subtitle segment due to missing timing data");
            return Ok(None);
        };

        if end < start {
            end = start;
        }

        let frame = segment.frame.clone();
        let region = segment.region;
        let engine = self.engine.clone();

        let response = tokio::task::spawn_blocking(move || {
            let plane = LumaPlane::from_frame(&frame);
            let regions = vec![to_ocr_region(&region, plane.width(), plane.height())];
            let request = OcrRequest::new(plane, &regions);
            engine.recognize(&request)
        })
        .await
        .map_err(|err| SubtitleGenError::Join(err.to_string()))?
        .map_err(SubtitleGenError::Ocr)?;

        let (text, confidence) = render_response(&response);
        if text.trim().is_empty() {
            return Ok(None);
        }

        if let Some(writer) = self.writer.as_ref() {
            writer
                .push(SubtitleEntry {
                    start,
                    end,
                    text: text.clone(),
                })
                .await
                .map_err(SubtitleGenError::Writer)?;
        }

        Ok(Some(GeneratedSubtitle {
            start,
            end,
            text,
            confidence,
            frame_index: segment.frame.frame_index(),
        }))
    }
}

fn to_ocr_region(region: &RoiConfig, width: u32, height: u32) -> OcrRegion {
    let mut x = region.x;
    let mut y = region.y;
    let mut w = region.width;
    let mut h = region.height;

    let max_w = width as f32;
    let max_h = height as f32;

    if w <= 0.0 || h <= 0.0 {
        x = 0.0;
        y = 0.0;
        w = max_w;
        h = max_h;
    }

    let x1 = (x + w).clamp(0.0, max_w);
    let y1 = (y + h).clamp(0.0, max_h);
    let x0 = x.clamp(0.0, max_w);
    let y0 = y.clamp(0.0, max_h);

    OcrRegion::new(x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0))
}

fn render_response(response: &OcrResponse) -> (String, Option<f32>) {
    use std::cmp::Ordering;

    if response.texts.is_empty() {
        return (String::new(), None);
    }

    let mut entries = response.texts.clone();
    entries.sort_by(|a, b| {
        a.region
            .y
            .partial_cmp(&b.region.y)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                a.region
                    .x
                    .partial_cmp(&b.region.x)
                    .unwrap_or(Ordering::Equal)
            })
    });

    let mut lines: Vec<(f32, Vec<String>)> = Vec::new();
    for item in entries {
        let trimmed = item.text.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((baseline, words)) = lines.last_mut() {
            if (item.region.y - *baseline).abs() <= LINE_MERGE_THRESHOLD_PX {
                words.push(trimmed.to_string());
                continue;
            }
        }
        lines.push((item.region.y, vec![trimmed.to_string()]));
    }

    let text = lines
        .into_iter()
        .map(|(_, words)| words.join(" "))
        .collect::<Vec<_>>()
        .join("\n");

    let mut sum = 0.0f32;
    let mut count = 0usize;
    for entry in &response.texts {
        if let Some(conf) = entry.confidence {
            sum += conf;
            count += 1;
        }
    }
    let confidence = if count > 0 {
        Some(sum / count as f32)
    } else {
        None
    };

    (text, confidence)
}

#[derive(Clone)]
struct SubtitleEntry {
    start: Duration,
    end: Duration,
    text: String,
}

struct SubtitleWriter {
    state: Mutex<SubtitleWriterState>,
}

struct SubtitleWriterState {
    path: PathBuf,
    entries: Vec<SubtitleEntry>,
    finalized: bool,
}

impl SubtitleWriter {
    fn new(path: PathBuf) -> Self {
        Self {
            state: Mutex::new(SubtitleWriterState {
                path,
                entries: Vec::new(),
                finalized: false,
            }),
        }
    }

    async fn push(&self, entry: SubtitleEntry) -> io::Result<()> {
        let mut state = self.state.lock().await;
        if state.finalized {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "attempted to push entry after finalize",
            ));
        }
        state.entries.push(entry);
        Ok(())
    }

    async fn finalize(&self) -> io::Result<()> {
        let mut state = self.state.lock().await;
        if state.finalized {
            return Ok(());
        }
        state
            .entries
            .sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.end.cmp(&b.end)));
        let path = state.path.clone();
        let entries = std::mem::take(&mut state.entries);
        state.finalized = true;
        drop(state);

        tokio::task::spawn_blocking(move || write_srt_file(&path, &entries))
            .await
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))??;
        Ok(())
    }
}

fn write_srt_file(path: &PathBuf, entries: &[SubtitleEntry]) -> io::Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut writer = std::io::BufWriter::new(std::fs::File::create(path)?);
    for (idx, entry) in entries.iter().enumerate() {
        write!(writer, "{}\r\n", idx + 1)?;
        write!(
            writer,
            "{} --> {}\r\n",
            format_timestamp(entry.start),
            format_timestamp(entry.end)
        )?;
        write!(writer, "{}\r\n\r\n", normalize_text(&entry.text))?;
    }
    writer.flush()?;
    Ok(())
}

fn format_timestamp(time: Duration) -> String {
    let total_millis = time.as_millis() as u64;
    let millis = (total_millis % 1_000) as u32;
    let total_seconds = total_millis / 1_000;
    let seconds = (total_seconds % 60) as u64;
    let minutes = ((total_seconds / 60) % 60) as u64;
    let hours = (total_seconds / 3_600) as u64;
    format!("{:02}:{:02}:{:02},{:03}", hours, minutes, seconds, millis)
}

fn normalize_text(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut normalized = text.replace("\r\n", "\n");
    normalized = normalized.replace('\r', "\n");
    while normalized.ends_with('\n') {
        normalized.pop();
    }
    normalized.replace('\n', "\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use subtitle_fast_ocr::{OcrRegion, OcrResponse, OcrText};

    #[tokio::test]
    async fn subtitle_writer_sorts_and_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.srt");
        let writer = SubtitleWriter::new(path.clone());
        writer
            .push(SubtitleEntry {
                start: Duration::from_secs(5),
                end: Duration::from_secs(6),
                text: "late".into(),
            })
            .await
            .unwrap();
        writer
            .push(SubtitleEntry {
                start: Duration::from_secs(2),
                end: Duration::from_secs(3),
                text: "early".into(),
            })
            .await
            .unwrap();
        writer.finalize().await.unwrap();
        let contents = std::fs::read_to_string(path).unwrap();
        assert!(contents.starts_with("1\r\n00:00:02,000 --> 00:00:03,000\r\nearly"));
    }

    #[test]
    fn timestamp_formatting_is_correct() {
        let ts = Duration::from_millis(3_726_045);
        assert_eq!(format_timestamp(ts), "01:02:06,045");
    }

    #[test]
    fn normalize_text_converts_line_endings() {
        let text = "hello\r\nworld\n";
        assert_eq!(normalize_text(text), "hello\r\nworld");
    }

    #[test]
    fn render_response_groups_lines() {
        let response = OcrResponse::new(vec![
            OcrText::new(OcrRegion::new(5.0, 10.0, 10.0, 4.0), "Hello".into()),
            OcrText::new(OcrRegion::new(60.0, 12.0, 10.0, 4.0), "World".into()),
            OcrText::new(OcrRegion::new(10.0, 40.0, 10.0, 4.0), "Line2".into()),
        ]);
        let (text, _) = render_response(&response);
        assert_eq!(text, "Hello World\nLine2");
    }
}
