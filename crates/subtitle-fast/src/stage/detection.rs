use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::f32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde::Serialize;
use tokio::fs;
use tokio::sync::mpsc;

use super::sampler::{FrameHistory, SampledFrame, SamplerContext, SamplerResult};
use super::{PipelineStage, StageInput, StageOutput};
use crate::settings::{ImageDumpSettings, JsonDumpSettings};
use crate::tools::YPlaneSaver;
use subtitle_fast_decoder::{YPlaneError, YPlaneFrame};
use subtitle_fast_validator::FrameValidator;
use subtitle_fast_validator::subtitle_detection::SubtitleDetectorKind;
use subtitle_fast_validator::subtitle_detection::{
    DetectionRegion, RoiConfig, SubtitleDetectionError, SubtitleDetectionResult,
};

const STABILITY_IOU_THRESHOLD: f32 = 0.6;
const ROI_EXPANSION_PX: f32 = 6.0;

pub const FAST_DETECTOR_KIND: SubtitleDetectorKind = SubtitleDetectorKind::LumaBand;
pub const PRECISE_DETECTOR_KIND: SubtitleDetectorKind = SubtitleDetectorKind::Auto;

pub type SubtitleStageResult = Result<SubtitleSegment, SubtitleStageError>;

pub trait SubtitleBandStrategy: Send + Sync {
    fn compute_span(&self, fps: Option<f64>, samples_per_second: u32) -> usize;
}

#[derive(Default)]
pub struct DefaultSubtitleBandStrategy;

impl SubtitleBandStrategy for DefaultSubtitleBandStrategy {
    fn compute_span(&self, fps: Option<f64>, samples_per_second: u32) -> usize {
        let Some(fps) = fps else {
            return 1;
        };
        if !fps.is_finite() || fps <= 0.0 {
            return 1;
        }
        let samples = samples_per_second.max(1) as usize;
        let frames_per_sample = (fps / samples as f64).round().max(1.0) as usize;
        frames_per_sample.min(samples).max(1)
    }
}

#[derive(Debug)]
pub enum SubtitleStageError {
    Decoder { error: YPlaneError, processed: u64 },
    Detection(SubtitleDetectionError),
}

pub struct SubtitleDetectionStage {
    fast_validator: FrameValidator,
    precise_validator: FrameValidator,
    samples_per_second: u32,
    strategy: Arc<dyn SubtitleBandStrategy>,
    image_settings: Option<ImageDumpSettings>,
    json_settings: Option<JsonDumpSettings>,
}

impl SubtitleDetectionStage {
    pub fn new(
        fast_validator: FrameValidator,
        precise_validator: FrameValidator,
        samples_per_second: u32,
        strategy: Arc<dyn SubtitleBandStrategy>,
        image_settings: Option<ImageDumpSettings>,
        json_settings: Option<JsonDumpSettings>,
    ) -> Self {
        Self {
            fast_validator,
            precise_validator,
            samples_per_second,
            strategy,
            image_settings,
            json_settings,
        }
    }
}

impl PipelineStage<SamplerResult> for SubtitleDetectionStage {
    type Output = SubtitleStageResult;

    fn name(&self) -> &'static str {
        "subtitle_detection"
    }

    fn apply(self: Box<Self>, input: StageInput<SamplerResult>) -> StageOutput<Self::Output> {
        let StageInput {
            stream,
            total_frames,
        } = input;

        let fast_validator = self.fast_validator.clone();
        let precise_validator = self.precise_validator.clone();
        let samples_per_second = self.samples_per_second;
        let strategy = self.strategy.clone();
        let image_settings = self.image_settings.clone();
        let json_settings = self.json_settings.clone();
        let (tx, rx) = mpsc::channel::<SubtitleStageResult>(24);

        tokio::spawn(async move {
            let mut upstream = stream;
            let mut worker = SubtitleDetectionWorker::new(
                fast_validator,
                precise_validator,
                samples_per_second,
                strategy,
                image_settings,
                json_settings,
            );

            loop {
                let item = upstream.next().await;
                match item {
                    Some(Ok(sample)) => match worker.handle_sample(sample).await {
                        Ok(Some(segment)) => {
                            if tx.send(Ok(segment)).await.is_err() {
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(err) => {
                            let _ = tx.send(Err(err)).await;
                            break;
                        }
                    },
                    Some(Err(err)) => {
                        let processed = worker.processed_samples;
                        let _ = tx
                            .send(Err(SubtitleStageError::Decoder {
                                error: err,
                                processed,
                            }))
                            .await;
                        break;
                    }
                    None => break,
                }
            }

            if let Some(segment) = worker.finalize_active().await {
                let _ = tx.send(Ok(segment)).await;
            }

            if let Err(err) = worker.finalize_outputs().await {
                eprintln!("debug finalize error: {err}");
            }

            worker.fast_validator.finalize().await;
            worker.precise_validator.finalize().await;
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

#[derive(Clone)]
struct DetectionShot {
    frame_index: u64,
    timestamp: Option<Duration>,
    frame: YPlaneFrame,
    region: DetectionRegion,
    regions: Vec<DetectionRegion>,
    score: f32,
}

struct SampleObservation {
    shot: DetectionShot,
    history: FrameHistory,
}

#[derive(Clone)]
struct TrackedRegion {
    frame_index: u64,
    region: DetectionRegion,
}

#[derive(Clone)]
struct StableTrack {
    regions: Vec<TrackedRegion>,
    score: f32,
}

struct ActiveSubtitle {
    roi: RoiConfig,
    start_timestamp: Option<Duration>,
    start_frame_index: Option<u64>,
    best_shot: DetectionShot,
    last_positive_shot: DetectionShot,
    regions: Vec<DetectionRegion>,
    debug: Option<SegmentDebugInfo>,
}

struct SubtitleDetectionWorker {
    fast_validator: FrameValidator,
    precise_validator: FrameValidator,
    window: VecDeque<SampleObservation>,
    required_consecutive: usize,
    active: Option<ActiveSubtitle>,
    frame_dimensions: Option<(u32, u32)>,
    samples_per_second: u32,
    strategy: Arc<dyn SubtitleBandStrategy>,
    processed_samples: u64,
    debug: DebugOutputs,
}

impl SubtitleDetectionWorker {
    fn new(
        fast_validator: FrameValidator,
        precise_validator: FrameValidator,
        samples_per_second: u32,
        strategy: Arc<dyn SubtitleBandStrategy>,
        image_settings: Option<ImageDumpSettings>,
        json_settings: Option<JsonDumpSettings>,
    ) -> Self {
        Self {
            fast_validator,
            precise_validator,
            window: VecDeque::new(),
            required_consecutive: 1,
            active: None,
            frame_dimensions: None,
            samples_per_second,
            strategy,
            processed_samples: 0,
            debug: DebugOutputs::new(image_settings, json_settings),
        }
    }

    fn update_required_span(&mut self, context: &SamplerContext) {
        let span = self
            .strategy
            .compute_span(context.estimated_fps(), self.samples_per_second)
            .max(1);
        self.required_consecutive = span;
    }

    async fn emit_frame(
        &mut self,
        frame: &YPlaneFrame,
        detection: &SubtitleDetectionResult,
        roi: Option<RoiConfig>,
    ) {
        self.debug.record_frame(frame, detection, roi).await;
    }

    async fn finalize_outputs(&mut self) -> std::io::Result<()> {
        self.debug.finalize().await
    }

    async fn filter_tracks_with_precise(
        &self,
        last_shot: &DetectionShot,
        tracks: Vec<StableTrack>,
        frame_dims: (u32, u32),
    ) -> Result<Vec<StableTrack>, SubtitleStageError> {
        let mut filtered = Vec::new();
        for mut track in tracks {
            let Some(last_entry) = track.regions.last() else {
                continue;
            };
            let roi = roi_with_margin(last_entry.region.clone(), frame_dims.0, frame_dims.1);
            let detection = self
                .precise_validator
                .process_frame_with_roi(last_shot.frame.clone(), Some(roi))
                .await
                .map_err(SubtitleStageError::Detection)?;
            println!(
                "[precise] frame={} roi=({}, {}, {}, {}) has_subtitle={}",
                last_shot.frame_index, roi.x, roi.y, roi.width, roi.height, detection.has_subtitle
            );
            if detection.has_subtitle {
                if let Some(precise_region) = best_region(&detection) {
                    if let Some(last) = track.regions.last_mut() {
                        last.region = precise_region;
                    }
                }
                filtered.push(track);
            }
        }
        Ok(filtered)
    }

    async fn handle_sample(
        &mut self,
        sample: SampledFrame,
    ) -> Result<Option<SubtitleSegment>, SubtitleStageError> {
        self.processed_samples = self.processed_samples.saturating_add(1);
        self.update_required_span(sample.sampler_context());

        let history = sample.history().clone();

        let frame_index = sample.frame_index;
        let frame = sample.frame().clone();
        let timestamp = sample.frame().timestamp();
        let dims = (frame.width(), frame.height());
        self.frame_dimensions = Some(dims);

        let roi = self.active.as_ref().map(|active| active.roi);

        let detection = self
            .fast_validator
            .process_frame_with_roi(frame.clone(), roi)
            .await
            .map_err(SubtitleStageError::Detection)?;

        if matches!(frame_index, 3590 | 3593 | 3597) {
            println!(
                "debug detection frame {}: has_subtitle={}, max_score={}",
                frame_index, detection.has_subtitle, detection.max_score
            );
        }

        self.emit_frame(&frame, &detection, roi).await;

        if detection.has_subtitle {
            self.handle_positive(frame_index, timestamp, frame, detection, history)
                .await
        } else {
            self.handle_negative(frame_index, timestamp, history).await
        }
    }

    async fn handle_positive(
        &mut self,
        frame_index: u64,
        timestamp: Option<Duration>,
        frame: YPlaneFrame,
        detection: SubtitleDetectionResult,
        history: FrameHistory,
    ) -> Result<Option<SubtitleSegment>, SubtitleStageError> {
        let Some(region) = best_region(&detection) else {
            return Ok(None);
        };

        let shot = DetectionShot {
            frame_index,
            timestamp,
            frame,
            region,
            regions: detection.regions.clone(),
            score: detection.max_score,
        };

        let observation = SampleObservation {
            shot: shot.clone(),
            history,
        };

        if let Some(active) = self.active.as_mut() {
            active.last_positive_shot = shot.clone();
            update_best_shot(active, &shot);
            if let Some((width, height)) = self.frame_dimensions {
                let updated = merge_regions(&active.best_shot.region, &shot.region);
                active.roi = roi_with_margin(updated, width, height);
            }
            append_regions(&mut active.regions, &shot.regions);
            return Ok(None);
        }

        self.window.push_back(observation);
        while self.window.len() > self.required_consecutive {
            self.window.pop_front();
        }

        if self.window.len() < self.required_consecutive {
            return Ok(None);
        }

        let dims = match self.frame_dimensions {
            Some(value) => value,
            None => {
                self.window.pop_front();
                return Ok(None);
            }
        };

        let mut stable_tracks = stable_region_tracks(&self.window);
        if stable_tracks.is_empty() {
            self.window.pop_front();
            return Ok(None);
        }

        let last_observation = match self.window.back() {
            Some(observation) => observation,
            None => {
                self.window.pop_front();
                return Ok(None);
            }
        };

        stable_tracks = self
            .filter_tracks_with_precise(&last_observation.shot, stable_tracks, dims)
            .await?;

        if stable_tracks.is_empty() {
            self.window.pop_front();
            return Ok(None);
        }

        let roi_region = match merged_region_from_tracks(&stable_tracks) {
            Some(region) => region,
            None => {
                self.window.pop_front();
                return Ok(None);
            }
        };
        let roi = roi_with_margin(roi_region, dims.0, dims.1);

        match self.determine_start(roi, &stable_tracks).await? {
            Some(active) => {
                self.active = Some(active);
                self.window.clear();
                Ok(None)
            }
            None => {
                self.window.pop_front();
                Ok(None)
            }
        }
    }

    async fn handle_negative(
        &mut self,
        _frame_index: u64,
        _timestamp: Option<Duration>,
        history: FrameHistory,
    ) -> Result<Option<SubtitleSegment>, SubtitleStageError> {
        if let Some(mut active) = self.active.take() {
            let (end_shot, best_update) = self.find_end(&mut active, &history).await?;
            if let Some(update) = best_update {
                update_best_shot(&mut active, &update);
            }
            if let Some(summary) = end_shot {
                let segment = SubtitleSegment {
                    frame: active.best_shot.frame.clone(),
                    max_score: active.best_shot.score,
                    region: roi_from_region(&active.best_shot.region),
                    start: active.start_timestamp,
                    end: summary.timestamp,
                    start_frame_index: active.start_frame_index,
                    end_frame_index: Some(summary.frame_index),
                    regions: active.regions.clone(),
                    debug: active.debug.clone(),
                };
                self.window.clear();
                return Ok(Some(segment));
            }
        }

        self.window.clear();
        Ok(None)
    }

    async fn finalize_active(&mut self) -> Option<SubtitleSegment> {
        if let Some(active) = self.active.take() {
            let segment = SubtitleSegment {
                frame: active.best_shot.frame.clone(),
                max_score: active.best_shot.score,
                region: roi_from_region(&active.best_shot.region),
                start: active.start_timestamp,
                end: active.last_positive_shot.timestamp,
                start_frame_index: active.start_frame_index,
                end_frame_index: Some(active.last_positive_shot.frame_index),
                regions: active.regions.clone(),
                debug: active.debug,
            };
            return Some(segment);
        }
        None
    }

    async fn determine_start(
        &mut self,
        roi: RoiConfig,
        tracks: &[StableTrack],
    ) -> Result<Option<ActiveSubtitle>, SubtitleStageError> {
        let first = match self.window.front() {
            Some(obs) => obs,
            None => return Ok(None),
        };
        let window_frames: Vec<u64> = self.window.iter().map(|obs| obs.shot.frame_index).collect();
        let last = match self.window.back() {
            Some(obs) => obs,
            None => return Ok(None),
        };

        let first_shot = first.shot.clone();
        let first_history = first.history.clone();
        let mut last_shot = last.shot.clone();

        let mut region_by_frame: HashMap<u64, DetectionRegion> = HashMap::new();
        for track in tracks {
            for entry in &track.regions {
                region_by_frame
                    .entry(entry.frame_index)
                    .and_modify(|existing| {
                        if entry.region.score > existing.score {
                            *existing = entry.region.clone();
                        }
                    })
                    .or_insert_with(|| entry.region.clone());
            }
        }

        let mut combined_regions = Vec::new();
        for track in tracks {
            for entry in &track.regions {
                combined_regions.push(entry.region.clone());
            }
        }

        let mut best_shot: Option<DetectionShot> = None;
        for observation in &self.window {
            let Some(region) = region_by_frame.get(&observation.shot.frame_index) else {
                continue;
            };
            let mut shot = observation.shot.clone();
            shot.region = region.clone();
            shot.score = region.score;
            shot.regions = vec![region.clone()];
            if best_shot
                .as_ref()
                .map(|existing| existing.score < shot.score)
                .unwrap_or(true)
            {
                best_shot = Some(shot);
            }
        }

        let mut best_shot = best_shot.unwrap_or_else(|| {
            let mut shot = first_shot.clone();
            if let Some(region) = region_by_frame.get(&shot.frame_index) {
                shot.region = region.clone();
                shot.score = region.score;
                shot.regions = vec![region.clone()];
            }
            shot
        });

        if let Some(region) = region_by_frame.get(&last_shot.frame_index) {
            last_shot.region = region.clone();
            last_shot.score = region.score;
            last_shot.regions = vec![region.clone()];
        }

        let mut pool_indices = Vec::new();
        for observation in &self.window {
            pool_indices.extend(
                observation
                    .history
                    .records()
                    .iter()
                    .map(|record| record.frame_index),
            );
            pool_indices.push(observation.shot.frame_index);
        }
        pool_indices.sort_unstable();
        pool_indices.dedup();

        let history_records = first_history
            .records()
            .iter()
            .map(|record| record.frame_index)
            .collect::<Vec<_>>();

        let mut positives = Vec::new();
        let history_shot = self
            .scan_history_backward(
                &first_history,
                roi,
                &mut combined_regions,
                Some(&mut positives),
            )
            .await?;

        let debug_info = SegmentDebugInfo {
            window: window_frames.clone(),
            pool: pool_indices,
            history: history_records,
            positives,
            selected: history_shot.as_ref().map(|shot| shot.frame_index),
        };
        let (start_timestamp, start_frame_index);
        if let Some(history_shot) = history_shot {
            if history_shot.score > best_shot.score {
                best_shot = history_shot.clone();
            }
            start_timestamp = history_shot.timestamp;
            start_frame_index = Some(history_shot.frame_index);
            append_regions(&mut combined_regions, &history_shot.regions);
        } else {
            start_timestamp = first_shot.timestamp;
            start_frame_index = Some(first_shot.frame_index);
        }
        append_regions(&mut combined_regions, &best_shot.regions);

        Ok(Some(ActiveSubtitle {
            roi,
            start_timestamp,
            start_frame_index,
            best_shot,
            last_positive_shot: last_shot,
            regions: combined_regions,
            debug: Some(debug_info),
        }))
    }

    async fn find_end(
        &mut self,
        active: &mut ActiveSubtitle,
        history: &FrameHistory,
    ) -> Result<(Option<DetectionShot>, Option<DetectionShot>), SubtitleStageError> {
        let last_positive_index = active.last_positive_shot.frame_index;
        let mut summary = active.last_positive_shot.clone();
        let mut best_update: Option<DetectionShot> = None;
        let mut prev_region = active.last_positive_shot.region.clone();

        for record in history.records() {
            if record.frame_index <= last_positive_index {
                continue;
            }
            let frame = record.frame_handle();
            let frame_clone = frame.as_ref().clone();
            let detection = self
                .fast_validator
                .process_frame_with_roi(frame_clone.clone(), Some(active.roi))
                .await
                .map_err(SubtitleStageError::Detection)?;
            self.emit_frame(&frame_clone, &detection, Some(active.roi))
                .await;
            if detection.has_subtitle {
                if let Some(region) = best_region(&detection) {
                    let iou = region_iou(&prev_region, &region);
                    if iou < STABILITY_IOU_THRESHOLD {
                        break;
                    }
                    let shot = DetectionShot {
                        frame_index: record.frame_index,
                        timestamp: frame_clone.timestamp(),
                        frame: frame_clone.clone(),
                        region,
                        regions: detection.regions.clone(),
                        score: detection.max_score,
                    };
                    summary = shot.clone();
                    prev_region = shot.region.clone();
                    append_regions(&mut active.regions, &detection.regions);
                    if detection.max_score > active.best_shot.score {
                        best_update = Some(shot.clone());
                    }
                }
            } else {
                break;
            }
        }

        Ok((Some(summary), best_update))
    }

    async fn scan_history_backward(
        &mut self,
        history: &FrameHistory,
        roi: RoiConfig,
        regions: &mut Vec<DetectionRegion>,
        mut debug_indices: Option<&mut Vec<u64>>,
    ) -> Result<Option<DetectionShot>, SubtitleStageError> {
        let mut last_positive: Option<DetectionShot> = None;
        let mut prev_region: Option<DetectionRegion> = None;

        for record in history.records().iter().rev() {
            let frame = record.frame_handle();
            let frame_clone = frame.as_ref().clone();
            let detection = self
                .fast_validator
                .process_frame_with_roi(frame_clone.clone(), Some(roi))
                .await
                .map_err(SubtitleStageError::Detection)?;
            self.emit_frame(&frame_clone, &detection, Some(roi)).await;

            if !detection.has_subtitle {
                break;
            }
            if let Some(region) = best_region(&detection) {
                if let Some(previous) = prev_region.as_ref() {
                    let iou = region_iou(previous, &region);
                    if iou < STABILITY_IOU_THRESHOLD {
                        break;
                    }
                }
                let shot = DetectionShot {
                    frame_index: record.frame_index,
                    timestamp: frame_clone.timestamp(),
                    frame: frame_clone.clone(),
                    region,
                    regions: detection.regions.clone(),
                    score: detection.max_score,
                };
                if let Some(indices) = debug_indices.as_deref_mut() {
                    indices.push(shot.frame_index);
                }
                append_regions(regions, &detection.regions);
                let region_clone = shot.region.clone();
                if last_positive
                    .as_ref()
                    .map_or(true, |existing| shot.frame_index < existing.frame_index)
                {
                    last_positive = Some(shot);
                }
                prev_region = Some(region_clone);
            } else {
                break;
            }
        }

        Ok(last_positive)
    }
}

struct DebugOutputs {
    image: Option<YPlaneSaver>,
    json: Option<JsonSink>,
}

impl DebugOutputs {
    fn new(
        image_settings: Option<ImageDumpSettings>,
        json_settings: Option<JsonDumpSettings>,
    ) -> Self {
        let image = image_settings.map(|settings| YPlaneSaver::new(settings.dir, settings.format));
        let json = json_settings.map(JsonSink::new);
        Self { image, json }
    }

    async fn record_frame(
        &mut self,
        frame: &YPlaneFrame,
        detection: &SubtitleDetectionResult,
        roi: Option<RoiConfig>,
    ) {
        if let Some(saver) = self.image.as_ref() {
            let index = frame_identifier(frame);
            if let Err(err) = saver.save(frame, detection, roi, index).await {
                eprintln!("frame dump error: {err}");
            }
        }
        if let Some(json) = self.json.as_mut() {
            json.record_frame(frame, detection, roi);
        }
    }

    async fn finalize(&mut self) -> std::io::Result<()> {
        if let Some(json) = self.json.as_mut() {
            json.flush().await?;
        }
        Ok(())
    }
}

struct JsonSink {
    frames: Vec<FrameEntry>,
    dir: PathBuf,
    frames_name: String,
    pretty: bool,
}

impl JsonSink {
    fn new(settings: JsonDumpSettings) -> Self {
        Self {
            frames: Vec::new(),
            dir: settings.dir,
            frames_name: settings.frames_filename,
            pretty: settings.pretty,
        }
    }

    fn record_frame(
        &mut self,
        frame: &YPlaneFrame,
        detection: &SubtitleDetectionResult,
        roi: Option<RoiConfig>,
    ) {
        self.frames.push(FrameEntry {
            frame_index: frame.frame_index(),
            timestamp: frame.timestamp().map(duration_secs),
            width: frame.width(),
            height: frame.height(),
            has_subtitle: detection.has_subtitle,
            max_score: detection.max_score,
            roi: roi.map(RoiEntry::from),
            regions: detection.regions.clone(),
        });
    }

    async fn flush(&mut self) -> std::io::Result<()> {
        if self.frames.is_empty() {
            return Ok(());
        }

        self.frames
            .sort_by_key(|entry| entry.frame_index.unwrap_or(u64::MAX));

        fs::create_dir_all(&self.dir).await?;

        let frames_path = self.dir.join(&self.frames_name);

        let frames_data = if self.pretty {
            serde_json::to_vec_pretty(&self.frames).map_err(json_error_to_io)?
        } else {
            serde_json::to_vec(&self.frames).map_err(json_error_to_io)?
        };

        fs::write(frames_path, frames_data).await?;
        Ok(())
    }
}

#[derive(Serialize)]
struct FrameEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_index: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamp: Option<f64>,
    width: u32,
    height: u32,
    has_subtitle: bool,
    max_score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    roi: Option<RoiEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    regions: Vec<DetectionRegion>,
}

#[derive(Serialize, Clone, Copy)]
struct RoiEntry {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl From<RoiConfig> for RoiEntry {
    fn from(value: RoiConfig) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        }
    }
}

fn json_error_to_io(err: serde_json::Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, err)
}

#[derive(Debug, Clone)]
pub struct SubtitleSegment {
    pub frame: YPlaneFrame,
    pub max_score: f32,
    pub region: RoiConfig,
    pub start: Option<Duration>,
    pub end: Option<Duration>,
    pub start_frame_index: Option<u64>,
    pub end_frame_index: Option<u64>,
    pub regions: Vec<DetectionRegion>,
    pub debug: Option<SegmentDebugInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SegmentDebugInfo {
    pub window: Vec<u64>,
    pub pool: Vec<u64>,
    pub history: Vec<u64>,
    pub positives: Vec<u64>,
    pub selected: Option<u64>,
}

fn frame_identifier(frame: &YPlaneFrame) -> u64 {
    frame
        .frame_index()
        .or_else(|| frame.timestamp().map(duration_millis))
        .unwrap_or_default()
}

fn duration_secs(value: Duration) -> f64 {
    value.as_secs_f64()
}

fn duration_millis(value: Duration) -> u64 {
    value.as_millis() as u64
}

fn best_region(result: &SubtitleDetectionResult) -> Option<DetectionRegion> {
    result
        .regions
        .iter()
        .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(Ordering::Equal))
        .cloned()
}

fn stable_region_tracks(window: &VecDeque<SampleObservation>) -> Vec<StableTrack> {
    if window.is_empty() {
        return Vec::new();
    }

    let mut iter = window.iter();
    let first = match iter.next() {
        Some(observation) => observation,
        None => return Vec::new(),
    };

    let mut tracks: Vec<StableTrack> = tracked_regions_for_observation(first)
        .into_iter()
        .map(|region| StableTrack {
            score: region.region.score,
            regions: vec![region],
        })
        .collect();

    if tracks.is_empty() {
        return Vec::new();
    }

    for observation in iter {
        let candidates = tracked_regions_for_observation(observation);
        if candidates.is_empty() {
            return Vec::new();
        }

        let mut next_tracks: Vec<StableTrack> = Vec::new();

        for track in &tracks {
            let Some(last_entry) = track.regions.last() else {
                continue;
            };

            let mut extended = false;
            for candidate in &candidates {
                if region_iou(&last_entry.region, &candidate.region) >= STABILITY_IOU_THRESHOLD {
                    let mut new_track = track.clone();
                    new_track.regions.push(candidate.clone());
                    new_track.score += candidate.region.score;
                    next_tracks.push(new_track);
                    extended = true;
                }
            }

            if !extended {
                // Track cannot be extended across the full window; drop it.
            }
        }

        if next_tracks.is_empty() {
            return Vec::new();
        }

        let mut deduped: Vec<StableTrack> = Vec::new();
        for track in next_tracks {
            let last = track
                .regions
                .last()
                .expect("stable track must contain at least one region");
            let mut merged = false;
            for existing in &mut deduped {
                let existing_last = existing
                    .regions
                    .last()
                    .expect("stable track must contain at least one region");
                if region_iou(&existing_last.region, &last.region) >= STABILITY_IOU_THRESHOLD {
                    if track.score > existing.score {
                        *existing = track.clone();
                    }
                    merged = true;
                    break;
                }
            }
            if !merged {
                deduped.push(track);
            }
        }

        tracks = deduped;
    }

    tracks
}

fn tracked_regions_for_observation(observation: &SampleObservation) -> Vec<TrackedRegion> {
    if observation.shot.regions.is_empty() {
        vec![TrackedRegion {
            frame_index: observation.shot.frame_index,
            region: observation.shot.region.clone(),
        }]
    } else {
        observation
            .shot
            .regions
            .iter()
            .cloned()
            .map(|region| TrackedRegion {
                frame_index: observation.shot.frame_index,
                region,
            })
            .collect()
    }
}

fn region_iou(a: &DetectionRegion, b: &DetectionRegion) -> f32 {
    let ax0 = a.x;
    let ay0 = a.y;
    let ax1 = a.x + a.width;
    let ay1 = a.y + a.height;

    let bx0 = b.x;
    let by0 = b.y;
    let bx1 = b.x + b.width;
    let by1 = b.y + b.height;

    let ix0 = ax0.max(bx0);
    let iy0 = ay0.max(by0);
    let ix1 = ax1.min(bx1);
    let iy1 = ay1.min(by1);

    if ix1 <= ix0 || iy1 <= iy0 {
        return 0.0;
    }

    let inter = (ix1 - ix0) * (iy1 - iy0);
    let a_area = a.width * a.height;
    let b_area = b.width * b.height;
    let union = a_area + b_area - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

fn merged_region_from_tracks(tracks: &[StableTrack]) -> Option<DetectionRegion> {
    let mut merged: Option<DetectionRegion> = None;
    for track in tracks {
        for entry in &track.regions {
            merged = Some(match merged {
                Some(existing) => merge_regions(&existing, &entry.region),
                None => entry.region.clone(),
            });
        }
    }
    merged
}

fn merge_regions(a: &DetectionRegion, b: &DetectionRegion) -> DetectionRegion {
    let x0 = a.x.min(b.x);
    let y0 = a.y.min(b.y);
    let x1 = (a.x + a.width).max(b.x + b.width);
    let y1 = (a.y + a.height).max(b.y + b.height);
    DetectionRegion {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
        score: a.score.max(b.score),
    }
}

fn roi_with_margin(region: DetectionRegion, width: u32, height: u32) -> RoiConfig {
    let margin = ROI_EXPANSION_PX;
    let mut x0 = region.x - margin;
    let mut y0 = region.y - margin;
    let mut x1 = region.x + region.width + margin;
    let mut y1 = region.y + region.height + margin;

    if x0 < 0.0 {
        x0 = 0.0;
    }
    if y0 < 0.0 {
        y0 = 0.0;
    }

    let width_f = width as f32;
    let height_f = height as f32;

    if x1 > width_f {
        x1 = width_f;
    }
    if y1 > height_f {
        y1 = height_f;
    }

    let roi_width = (x1 - x0).max(1.0);
    let roi_height = (y1 - y0).max(1.0);
    let norm_width = if width_f > 0.0 {
        roi_width / width_f
    } else {
        1.0
    };
    let norm_height = if height_f > 0.0 {
        roi_height / height_f
    } else {
        1.0
    };
    let norm_x = if width_f > 0.0 { x0 / width_f } else { 0.0 };
    let norm_y = if height_f > 0.0 { y0 / height_f } else { 0.0 };

    RoiConfig {
        x: norm_x.clamp(0.0, 1.0),
        y: norm_y.clamp(0.0, 1.0),
        width: norm_width.clamp(f32::EPSILON, 1.0),
        height: norm_height.clamp(f32::EPSILON, 1.0),
    }
}

fn roi_from_region(region: &DetectionRegion) -> RoiConfig {
    RoiConfig {
        x: region.x,
        y: region.y,
        width: region.width.max(1.0),
        height: region.height.max(1.0),
    }
}

fn update_best_shot(active: &mut ActiveSubtitle, shot: &DetectionShot) {
    if shot.score > active.best_shot.score {
        active.best_shot = shot.clone();
    }
}

fn append_regions(target: &mut Vec<DetectionRegion>, source: &[DetectionRegion]) {
    if source.is_empty() {
        return;
    }
    target.extend(source.iter().cloned());
}
