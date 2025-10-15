use std::cmp::Ordering;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde::Serialize;
use tokio::fs;
use tokio::sync::{Mutex, mpsc};

use super::{PipelineStage, StageInput, StageOutput};
use crate::settings::JsonDumpSettings;
use crate::stage::detection::{SubtitleSegment, SubtitleStageError, SubtitleStageResult};
use subtitle_fast_decoder::YPlaneFrame;
use subtitle_fast_ocr::{LumaPlane, OcrEngine, OcrError, OcrRegion, OcrRequest, OcrResponse};
use subtitle_fast_validator::subtitle_detection::{DetectionRegion, RoiConfig};

const LINE_MERGE_THRESHOLD_PX: f32 = 12.0;
const OCR_SIDE_EXPAND_PX: f32 = 12.0;
const OCR_VERTICAL_EXPAND_PX: f32 = 4.0;
const SRT_MERGE_THRESHOLD: Duration = Duration::from_millis(40);

pub type SubtitleGenResult = Result<GeneratedSubtitle, SubtitleGenError>;

/// 生成字幕: converts detected subtitle segments into final text output.
pub struct SubtitleGen<E>
where
    E: OcrEngine + 'static + ?Sized,
{
    engine: Arc<E>,
    writer: Arc<SubtitleWriter>,
    segments: Option<Arc<SegmentsDump>>,
}

impl<E> SubtitleGen<E>
where
    E: OcrEngine + 'static + ?Sized,
{
    pub fn new(
        engine: Arc<E>,
        output_path: PathBuf,
        json_settings: Option<JsonDumpSettings>,
    ) -> Self {
        let writer = Arc::new(SubtitleWriter::new(output_path));
        let segments = json_settings.map(|settings| {
            Arc::new(SegmentsDump::new(
                settings.dir,
                settings.segments_filename,
                settings.pretty,
            ))
        });
        Self {
            engine,
            writer,
            segments,
        }
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
        let segments = self.segments.clone();
        let (tx, rx) = mpsc::channel::<SubtitleGenResult>(24);

        tokio::spawn(async move {
            let mut upstream = stream;
            let mut worker = SubtitleGenWorker::new(engine, writer, segments);

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

            if let Err(err) = worker.writer.finalize().await {
                let _ = tx.send(Err(SubtitleGenError::Writer(err))).await;
            }
            if let Some(json) = worker.segments.clone() {
                if let Err(err) = json.finalize().await {
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
    writer: Arc<SubtitleWriter>,
    segments: Option<Arc<SegmentsDump>>,
}

impl<E> SubtitleGenWorker<E>
where
    E: OcrEngine + 'static + ?Sized,
{
    fn new(
        engine: Arc<E>,
        writer: Arc<SubtitleWriter>,
        segments: Option<Arc<SegmentsDump>>,
    ) -> Self {
        Self {
            engine,
            writer,
            segments,
        }
    }

    async fn handle_segment(
        &mut self,
        segment: SubtitleSegment,
    ) -> Result<Option<GeneratedSubtitle>, SubtitleGenError> {
        let SubtitleSegment {
            frame,
            max_score,
            region,
            start: raw_start,
            end: raw_end,
            start_frame_index,
            end_frame_index,
            regions,
        } = segment;

        let frame_timestamp = frame.timestamp();
        let start = raw_start.or(frame_timestamp);
        let end = raw_end.or(frame_timestamp);

        let (Some(start), Some(mut end)) = (start, end) else {
            eprintln!("skipping subtitle segment due to missing timing data");
            return Ok(None);
        };

        if end < start {
            end = start;
        }

        let frame_width = frame.width();
        let frame_height = frame.height();
        let region_for_ocr = expand_regions_for_ocr(&region, &regions, frame_width, frame_height);
        let region_for_dump = copy_roi(&region_for_ocr);
        let frame_for_ocr = frame.clone();
        let engine = self.engine.clone();

        let response = tokio::task::spawn_blocking(move || {
            let plane = LumaPlane::from_frame(&frame_for_ocr);
            let regions = vec![to_ocr_region(
                &region_for_ocr,
                plane.width(),
                plane.height(),
            )];
            let request = OcrRequest::new(plane, &regions);
            engine.recognize(&request)
        })
        .await
        .map_err(|err| SubtitleGenError::Join(err.to_string()))?
        .map_err(SubtitleGenError::Ocr)?;

        let (raw_text, confidence) = render_response(&response);
        let trimmed_text = raw_text.trim();
        let text = trimmed_text.to_string();

        if let Some(json) = self.segments.as_ref() {
            let entry = SegmentDumpEntry::from_segment(
                &frame,
                start_frame_index,
                end_frame_index,
                start,
                end,
                max_score,
                text.as_str(),
                confidence,
                &region_for_dump,
            );
            json.record(entry).await.map_err(SubtitleGenError::Writer)?;
        }

        if text.is_empty() {
            return Ok(None);
        }

        self.writer
            .push(SubtitleEntry {
                start,
                end,
                text: text.clone(),
            })
            .await
            .map_err(SubtitleGenError::Writer)?;

        Ok(Some(GeneratedSubtitle {
            start,
            end,
            text,
            confidence,
            frame_index: frame.frame_index(),
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

fn expand_regions_for_ocr(
    primary: &RoiConfig,
    regions: &[DetectionRegion],
    frame_width: u32,
    frame_height: u32,
) -> RoiConfig {
    let mut bounds = RegionBounds::new(frame_width, frame_height);
    for region in regions {
        bounds.include(region.x, region.y, region.width, region.height);
    }
    bounds.include(primary.x, primary.y, primary.width, primary.height);
    bounds.expand(OCR_SIDE_EXPAND_PX, OCR_VERTICAL_EXPAND_PX);
    bounds.into_roi()
}

fn copy_roi(value: &RoiConfig) -> RoiConfig {
    RoiConfig {
        x: value.x,
        y: value.y,
        width: value.width,
        height: value.height,
    }
}

struct RegionBounds {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    frame_width: f32,
    frame_height: f32,
    has_value: bool,
}

impl RegionBounds {
    fn new(frame_width: u32, frame_height: u32) -> Self {
        Self {
            x0: frame_width as f32,
            y0: frame_height as f32,
            x1: 0.0,
            y1: 0.0,
            frame_width: frame_width as f32,
            frame_height: frame_height as f32,
            has_value: false,
        }
    }

    fn include(&mut self, x: f32, y: f32, width: f32, height: f32) {
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let (px, py, pw, ph) =
            to_pixel_rect(x, y, width, height, self.frame_width, self.frame_height);
        let frame_w = self.frame_width.max(1.0);
        let frame_h = self.frame_height.max(1.0);
        let x0 = px.max(0.0);
        let y0 = py.max(0.0);
        let mut x1 = (px + pw).min(frame_w);
        let mut y1 = (py + ph).min(frame_h);
        if x1 <= x0 {
            x1 = (x0 + 1.0).min(frame_w);
        }
        if y1 <= y0 {
            y1 = (y0 + 1.0).min(frame_h);
        }
        if !self.has_value {
            self.x0 = x0;
            self.y0 = y0;
            self.x1 = x1;
            self.y1 = y1;
            self.has_value = true;
        } else {
            self.x0 = self.x0.min(x0);
            self.y0 = self.y0.min(y0);
            self.x1 = self.x1.max(x1);
            self.y1 = self.y1.max(y1);
        }
    }

    fn expand(&mut self, horizontal: f32, vertical: f32) {
        if !self.has_value {
            return;
        }

        let frame_w = self.frame_width.max(1.0);
        let frame_h = self.frame_height.max(1.0);

        self.x0 = (self.x0 - horizontal).max(0.0);
        self.y0 = (self.y0 - vertical).max(0.0);
        self.x1 = (self.x1 + horizontal).min(frame_w);
        self.y1 = (self.y1 + vertical).min(frame_h);

        if (self.x1 - self.x0) < 1.0 {
            let deficit = 1.0 - (self.x1 - self.x0);
            let adjust = deficit * 0.5;
            self.x0 = (self.x0 - adjust).max(0.0);
            self.x1 = (self.x1 + adjust).min(frame_w);
            if (self.x1 - self.x0) < 1.0 {
                self.x1 = (self.x0 + 1.0).min(frame_w);
                self.x0 = (self.x1 - 1.0).max(0.0);
            }
        }

        if (self.y1 - self.y0) < 1.0 {
            let deficit = 1.0 - (self.y1 - self.y0);
            let adjust = deficit * 0.5;
            self.y0 = (self.y0 - adjust).max(0.0);
            self.y1 = (self.y1 + adjust).min(frame_h);
            if (self.y1 - self.y0) < 1.0 {
                self.y1 = (self.y0 + 1.0).min(frame_h);
                self.y0 = (self.y1 - 1.0).max(0.0);
            }
        }
    }

    fn into_roi(self) -> RoiConfig {
        if !self.has_value {
            return RoiConfig {
                x: 0.0,
                y: 0.0,
                width: self.frame_width.max(1.0),
                height: self.frame_height.max(1.0),
            };
        }

        RoiConfig {
            x: self.x0,
            y: self.y0,
            width: (self.x1 - self.x0).max(1.0),
            height: (self.y1 - self.y0).max(1.0),
        }
    }
}

fn to_pixel_rect(
    mut x: f32,
    mut y: f32,
    mut width: f32,
    mut height: f32,
    frame_width: f32,
    frame_height: f32,
) -> (f32, f32, f32, f32) {
    if frame_width.is_normal() && frame_height.is_normal() {
        if x >= 0.0
            && x <= 1.0
            && y >= 0.0
            && y <= 1.0
            && width > 0.0
            && width <= 1.0
            && height > 0.0
            && height <= 1.0
        {
            x *= frame_width;
            y *= frame_height;
            width *= frame_width;
            height *= frame_height;
        }
    }
    (x, y, width, height)
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

#[derive(Serialize)]
struct SegmentDumpEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    start_frame: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    end_frame: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    best_frame: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    end_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    best_time: Option<f64>,
    detection_score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    ocr_confidence: Option<f32>,
    text: String,
    region: RoiRect,
}

#[derive(Serialize)]
struct RoiRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl SegmentDumpEntry {
    fn from_segment(
        frame: &YPlaneFrame,
        start_frame: Option<u64>,
        end_frame: Option<u64>,
        start: Duration,
        end: Duration,
        detection_score: f32,
        text: &str,
        ocr_confidence: Option<f32>,
        region: &RoiConfig,
    ) -> Self {
        Self {
            start_frame,
            end_frame,
            best_frame: frame.frame_index(),
            start_time: Some(start.as_secs_f64()),
            end_time: Some(end.as_secs_f64()),
            best_time: frame.timestamp().map(|ts| ts.as_secs_f64()),
            detection_score,
            ocr_confidence,
            text: text.to_string(),
            region: RoiRect::from(region),
        }
    }

    fn frame_key(&self) -> u64 {
        self.start_frame
            .or(self.best_frame)
            .or(self.end_frame)
            .unwrap_or(u64::MAX)
    }

    fn time_key(&self) -> f64 {
        self.start_time
            .or(self.best_time)
            .or(self.end_time)
            .unwrap_or(f64::MAX)
    }
}

impl From<&RoiConfig> for RoiRect {
    fn from(value: &RoiConfig) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        }
    }
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
        if let Some(last) = state.entries.last_mut() {
            if should_merge_entries(last, &entry) {
                if entry.end > last.end {
                    last.end = entry.end;
                }
                return Ok(());
            }
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

struct SegmentsDump {
    state: Mutex<SegmentsDumpState>,
}

struct SegmentsDumpState {
    dir: PathBuf,
    filename: String,
    pretty: bool,
    entries: Vec<SegmentDumpEntry>,
    finalized: bool,
}

impl SegmentsDump {
    fn new(dir: PathBuf, filename: String, pretty: bool) -> Self {
        Self {
            state: Mutex::new(SegmentsDumpState {
                dir,
                filename,
                pretty,
                entries: Vec::new(),
                finalized: false,
            }),
        }
    }

    async fn record(&self, entry: SegmentDumpEntry) -> io::Result<()> {
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
        state.entries.sort_by(|a, b| {
            let frame_cmp = a.frame_key().cmp(&b.frame_key());
            if frame_cmp != Ordering::Equal {
                return frame_cmp;
            }
            a.time_key()
                .partial_cmp(&b.time_key())
                .unwrap_or(Ordering::Equal)
        });
        let dir = state.dir.clone();
        let filename = state.filename.clone();
        let pretty = state.pretty;
        let entries = std::mem::take(&mut state.entries);
        state.finalized = true;
        drop(state);

        fs::create_dir_all(&dir).await?;
        let path = dir.join(filename);
        let data = if pretty {
            serde_json::to_vec_pretty(&entries).map_err(json_error_to_io)?
        } else {
            serde_json::to_vec(&entries).map_err(json_error_to_io)?
        };
        fs::write(path, data).await?;
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

fn json_error_to_io(err: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::Other, err)
}

fn should_merge_entries(a: &SubtitleEntry, b: &SubtitleEntry) -> bool {
    if a.text != b.text {
        return false;
    }
    let (first, second) = if a.start <= b.start { (a, b) } else { (b, a) };
    let gap = if second.start >= first.end {
        second.start - first.end
    } else {
        Duration::ZERO
    };
    gap <= SRT_MERGE_THRESHOLD
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

    #[tokio::test]
    async fn subtitle_writer_merges_adjacent_matching_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("merged.srt");
        let writer = SubtitleWriter::new(path.clone());

        writer
            .push(SubtitleEntry {
                start: Duration::from_secs(1),
                end: Duration::from_secs(2),
                text: "merge".into(),
            })
            .await
            .unwrap();
        writer
            .push(SubtitleEntry {
                start: Duration::from_secs(2),
                end: Duration::from_secs(3),
                text: "merge".into(),
            })
            .await
            .unwrap();

        writer.finalize().await.unwrap();
        let contents = std::fs::read_to_string(path).unwrap();
        assert!(contents.contains("00:00:01,000 --> 00:00:03,000"));
        assert!(!contents.contains("2\r\n"));
    }
}
