use std::sync::Arc;

use crate::settings::{ImageDumpSettings, JsonDumpSettings};
use crate::stage::detection::SubtitleSegment;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

use super::error::OutputError;
use super::image::ImageOutput;
use super::json::JsonOutput;
use super::types::{
    FrameAnalysisSample, FrameJsonRecord, SegmentJsonRecord, frame_sort_key, segment_sort_key,
};

pub struct OutputManager {
    image: Option<ImageOutput>,
    json: Option<JsonOutput>,
    state: Mutex<OutputState>,
}

impl OutputManager {
    pub fn new(
        image: Option<ImageDumpSettings>,
        json: Option<JsonDumpSettings>,
    ) -> Option<Arc<Self>> {
        if image.is_none() && json.is_none() {
            return None;
        }
        Some(Arc::new(Self {
            image: image.map(ImageOutput::new),
            json: json.map(JsonOutput::new),
            state: Mutex::new(OutputState::default()),
        }))
    }

    pub fn spawn_frame_worker(
        self: &Arc<Self>,
    ) -> (UnboundedSender<FrameAnalysisSample>, JoinHandle<()>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let manager = Arc::clone(self);
        let handle = tokio::spawn(async move {
            manager.consume_frames(rx).await;
        });
        (tx, handle)
    }

    async fn consume_frames(self: Arc<Self>, mut rx: UnboundedReceiver<FrameAnalysisSample>) {
        while let Some(sample) = rx.recv().await {
            if let Err(err) = self.process_frame(sample).await {
                eprintln!("frame output error: {err}");
            }
        }
    }

    async fn process_frame(&self, sample: FrameAnalysisSample) -> Result<(), OutputError> {
        if let Some(image) = self.image.as_ref() {
            image.write(&sample).await?;
        }
        if self.json.is_some() {
            let summary = FrameJsonRecord::from_sample(&sample);
            let mut state = self.state.lock().await;
            state.frames.push(summary);
        }
        Ok(())
    }

    pub async fn record_segment(&self, segment: &SubtitleSegment) -> Result<(), OutputError> {
        if self.json.is_none() {
            return Ok(());
        }
        let summary = SegmentJsonRecord::from_segment(segment);
        let mut state = self.state.lock().await;
        state.segments.push(summary);
        Ok(())
    }

    pub async fn finalize(&self) -> Result<(), OutputError> {
        if let Some(json) = self.json.as_ref() {
            let mut state = self.state.lock().await;
            state
                .frames
                .sort_by(|a, b| frame_sort_key(a).cmp(&frame_sort_key(b)));
            state
                .segments
                .sort_by(|a, b| segment_sort_key(a).cmp(&segment_sort_key(b)));
            json.write(&state.frames, &state.segments).await?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct OutputState {
    frames: Vec<FrameJsonRecord>,
    segments: Vec<SegmentJsonRecord>,
}
