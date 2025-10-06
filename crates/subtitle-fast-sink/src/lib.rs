use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use image::codecs::jpeg::JpegEncoder;
use image::ColorType;
use serde::Serialize;
use subtitle_fast_decoder::YPlaneFrame;
use thiserror::Error;
use tokio::sync::{
    mpsc::{self, Sender},
    Mutex, Semaphore,
};
use tokio::task::{self, JoinHandle, JoinSet};

pub mod subtitle_detection;

use subtitle_detection::{
    SubtitleDetectionConfig, SubtitleDetectionResult, SubtitlePresenceDetector,
};

const DEFAULT_CHANNEL_CAPACITY: usize = 64;
const DEFAULT_MAX_CONCURRENCY: usize = 8;

#[derive(Clone, Debug)]
pub struct FrameSinkConfig {
    pub channel_capacity: usize,
    pub max_concurrency: usize,
    pub jpeg: Option<JpegConfig>,
    pub detection: SubtitleDetectionOptions,
}

impl Default for FrameSinkConfig {
    fn default() -> Self {
        Self {
            channel_capacity: DEFAULT_CHANNEL_CAPACITY,
            max_concurrency: DEFAULT_MAX_CONCURRENCY,
            jpeg: None,
            detection: SubtitleDetectionOptions::default(),
        }
    }
}

impl FrameSinkConfig {
    pub fn from_outputs(dump_dir: Option<PathBuf>, samples_per_second: u32) -> Self {
        let mut config = Self::default();
        if let Some(dir) = dump_dir {
            config.jpeg = Some(JpegConfig::new(dir));
        }
        if let Some(path) = std::env::var_os("SUBFAST_DEBUG_DETECTION_PATH") {
            config.detection.debug_output = Some(PathBuf::from(path));
        }
        config.detection.samples_per_second = samples_per_second.max(1);
        config
    }
}

#[derive(Clone, Debug)]
pub struct JpegConfig {
    pub directory: PathBuf,
    pub quality: u8,
}

impl JpegConfig {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            quality: 90,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SubtitleDetectionOptions {
    pub enabled: bool,
    pub debug_output: Option<PathBuf>,
    pub samples_per_second: u32,
}

impl Default for SubtitleDetectionOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            debug_output: None,
            samples_per_second: 7,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FrameMetadata {
    pub frame_index: u64,
    pub processed_index: u64,
    pub timestamp: Option<Duration>,
}

pub struct FrameSink {
    sender: Sender<Job>,
    worker: JoinHandle<()>,
}

struct Job {
    frame: YPlaneFrame,
    metadata: FrameMetadata,
}

impl FrameSink {
    pub fn new(config: FrameSinkConfig) -> Self {
        let capacity = config.channel_capacity.max(1);
        let concurrency = config.max_concurrency.max(1);
        let (tx, mut rx) = mpsc::channel::<Job>(capacity);
        let operations = Arc::new(ProcessingOperations::new(config));
        let semaphore = Arc::new(Semaphore::new(concurrency));

        let worker = tokio::spawn({
            let operations = Arc::clone(&operations);
            let semaphore = Arc::clone(&semaphore);
            async move {
                let mut tasks = JoinSet::new();
                while let Some(job) = rx.recv().await {
                    let operations = Arc::clone(&operations);
                    let semaphore = Arc::clone(&semaphore);
                    tasks.spawn(async move {
                        let permit = match semaphore.acquire_owned().await {
                            Ok(permit) => permit,
                            Err(err) => {
                                eprintln!("frame sink semaphore error: {err}");
                                return;
                            }
                        };

                        let _permit = permit;
                        let Job { frame, metadata } = job;
                        if let Some(jpeg) = operations.jpeg.as_ref() {
                            if let Err(err) = jpeg.process(&frame, &metadata).await {
                                eprintln!("frame sink jpeg error: {err}");
                            }
                        }
                        if let Some(detection) = operations.detection.as_ref() {
                            detection.process(&frame, &metadata).await;
                        }
                    });
                }

                while let Some(result) = tasks.join_next().await {
                    if let Err(err) = result {
                        if err.is_cancelled() {
                            continue;
                        }
                        eprintln!("frame sink join error: {err}");
                    }
                }

                if let Some(detection) = operations.detection.as_ref() {
                    detection.finalize().await;
                }
            }
        });

        Self { sender: tx, worker }
    }

    pub fn push(&self, frame: YPlaneFrame, metadata: FrameMetadata) -> bool {
        self.sender.try_send(Job { frame, metadata }).is_ok()
    }

    pub async fn shutdown(self) {
        let FrameSink { sender, worker } = self;
        drop(sender);
        let _ = worker.await;
    }
}

struct ProcessingOperations {
    jpeg: Option<Arc<JpegOperation>>,
    detection: Option<Arc<SubtitleDetectionOperation>>,
}

impl ProcessingOperations {
    fn new(config: FrameSinkConfig) -> Self {
        let jpeg = config
            .jpeg
            .map(|jpeg_cfg| Arc::new(JpegOperation::new(jpeg_cfg.directory, jpeg_cfg.quality)));
        let detection = if config.detection.enabled {
            Some(Arc::new(SubtitleDetectionOperation::new(config.detection)))
        } else {
            None
        };
        Self { jpeg, detection }
    }
}

struct JpegOperation {
    directory: Arc<PathBuf>,
    quality: u8,
}

impl JpegOperation {
    fn new(directory: PathBuf, quality: u8) -> Self {
        Self {
            directory: Arc::new(directory),
            quality,
        }
    }

    async fn process(
        &self,
        frame: &YPlaneFrame,
        metadata: &FrameMetadata,
    ) -> Result<(), WriteFrameError> {
        write_frame(frame, metadata.frame_index, &self.directory, self.quality).await
    }
}

struct SubtitleDetectionOperation {
    state: Mutex<SubtitleDetectionState>,
}

impl SubtitleDetectionOperation {
    fn new(options: SubtitleDetectionOptions) -> Self {
        Self {
            state: Mutex::new(SubtitleDetectionState::new(options)),
        }
    }

    async fn process(&self, frame: &YPlaneFrame, metadata: &FrameMetadata) {
        let mut state = self.state.lock().await;
        state.process_frame(frame, metadata);
    }

    async fn finalize(&self) {
        let mut state = self.state.lock().await;
        if let Err(err) = state.finalize() {
            eprintln!("failed to write subtitle detection debug output: {err}");
        }
    }
}

struct SubtitleDetectionState {
    detector: Option<SubtitlePresenceDetector>,
    detector_dims: Option<(usize, usize, usize)>,
    init_error_logged: bool,
    debug: Option<DetectionDebug>,
    sampling: SubtitleSamplingState,
}

impl SubtitleDetectionState {
    fn new(options: SubtitleDetectionOptions) -> Self {
        let debug = options.debug_output.map(|path| DetectionDebug {
            path,
            entries: Vec::new(),
        });
        Self {
            detector: None,
            detector_dims: None,
            init_error_logged: false,
            debug,
            sampling: SubtitleSamplingState::new(options.samples_per_second.max(1)),
        }
    }

    fn process_frame(&mut self, frame: &YPlaneFrame, metadata: &FrameMetadata) {
        let dims = (
            frame.width() as usize,
            frame.height() as usize,
            frame.stride(),
        );
        if self.detector_dims != Some(dims) {
            self.detector_dims = Some(dims);
            self.sampling.reset();
            match SubtitlePresenceDetector::new(SubtitleDetectionConfig::for_frame(
                dims.0, dims.1, dims.2,
            )) {
                Ok(detector) => {
                    self.detector = Some(detector);
                    self.init_error_logged = false;
                }
                Err(err) => {
                    if !self.init_error_logged {
                        eprintln!("subtitle detection initialization failed: {err}");
                        self.init_error_logged = true;
                    }
                    self.detector = None;
                    if let Some(debug) = self.debug.as_mut() {
                        debug.record_error(metadata, err.to_string());
                    }
                    return;
                }
            }
        }

        let Some(detector) = self.detector.as_ref() else {
            if let Some(debug) = self.debug.as_mut() {
                debug.record_error(metadata, "detector unavailable".to_string());
            }
            return;
        };

        let frame_clone = frame.clone();
        let metadata_clone = metadata.clone();
        let output = self
            .sampling
            .ingest_frame(frame_clone, metadata_clone, detector);
        self.handle_sampling_output(output);
    }

    fn handle_sampling_output(&mut self, output: SamplingOutput) {
        for failure in output.failures {
            eprintln!(
                "subtitle detection failed for frame {frame}: {message}",
                frame = failure.metadata.frame_index,
                message = failure.message
            );
        }
        self.record_frames(output.flushed);
    }

    fn record_frames(&mut self, frames: Vec<BufferedFrame>) {
        let Some(debug) = self.debug.as_mut() else {
            return;
        };

        for frame in frames {
            let metadata = frame.metadata;
            match frame.decision {
                FrameDecision::Evaluated(result) => {
                    debug.record_success(&metadata, &result);
                }
                FrameDecision::Assumed(has_subtitle) => {
                    debug.record_assumption(&metadata, has_subtitle);
                }
                FrameDecision::Error(message) => {
                    debug.record_error(&metadata, message);
                }
                FrameDecision::Pending => {
                    debug.record_error(
                        &metadata,
                        "subtitle detection pending without decision".to_string(),
                    );
                }
            }
        }
    }

    fn finalize(&mut self) -> std::io::Result<()> {
        if let Some(detector) = self.detector.as_ref() {
            let output = self.sampling.finalize(detector);
            self.handle_sampling_output(output);
        } else {
            self.sampling.reset();
        }

        if let Some(debug) = self.debug.take() {
            debug.write_json()?;
        }
        Ok(())
    }
}

#[derive(Clone)]
struct BufferedFrame {
    frame: YPlaneFrame,
    metadata: FrameMetadata,
    decision: FrameDecision,
}

#[derive(Clone)]
enum FrameDecision {
    Pending,
    Evaluated(SubtitleDetectionResult),
    Assumed(bool),
    Error(String),
}

impl FrameDecision {
    fn has_subtitle(&self) -> Option<bool> {
        match self {
            FrameDecision::Evaluated(result) => Some(result.has_subtitle),
            FrameDecision::Assumed(value) => Some(*value),
            FrameDecision::Error(_) | FrameDecision::Pending => None,
        }
    }
}

#[derive(Clone)]
struct DetectionFailure {
    metadata: FrameMetadata,
    message: String,
}

struct SamplingOutput {
    flushed: Vec<BufferedFrame>,
    failures: Vec<DetectionFailure>,
}

impl SamplingOutput {
    fn new() -> Self {
        Self {
            flushed: Vec::new(),
            failures: Vec::new(),
        }
    }

    fn extend_with_frames(&mut self, frames: Vec<BufferedFrame>) {
        self.flushed.extend(frames);
    }
}

struct SubtitleSamplingState {
    samples_per_second: u32,
    current_second: Option<SecondBuffer>,
}

impl SubtitleSamplingState {
    fn new(samples_per_second: u32) -> Self {
        Self {
            samples_per_second,
            current_second: None,
        }
    }

    fn reset(&mut self) {
        self.current_second = None;
    }

    fn ingest_frame(
        &mut self,
        frame: YPlaneFrame,
        metadata: FrameMetadata,
        detector: &SubtitlePresenceDetector,
    ) -> SamplingOutput {
        let mut output = SamplingOutput::new();

        let (second_index, second_start) =
            Self::resolve_second(&self.current_second, &frame, &metadata);

        if self
            .current_second
            .as_ref()
            .map(|buffer| buffer.second_index)
            != Some(second_index)
        {
            if let Some(buffer) = self.current_second.take() {
                let mut buffer = buffer;
                let frames = buffer.into_frames(detector, &mut output.failures);
                output.extend_with_frames(frames);
            }
            self.current_second = Some(SecondBuffer::new(
                second_index,
                second_start,
                self.samples_per_second,
            ));
        }

        if let Some(buffer) = self.current_second.as_mut() {
            buffer.push_frame(frame, metadata, detector, &mut output.failures);
        }

        output
    }

    fn finalize(&mut self, detector: &SubtitlePresenceDetector) -> SamplingOutput {
        let mut output = SamplingOutput::new();
        if let Some(buffer) = self.current_second.take() {
            let mut buffer = buffer;
            let frames = buffer.into_frames(detector, &mut output.failures);
            output.extend_with_frames(frames);
        }
        output
    }

    fn resolve_second(
        current: &Option<SecondBuffer>,
        frame: &YPlaneFrame,
        metadata: &FrameMetadata,
    ) -> (u64, Duration) {
        if let Some(timestamp) = metadata.timestamp.or_else(|| frame.timestamp()) {
            let secs = timestamp.as_secs();
            (secs, Duration::from_secs(secs))
        } else if let Some(buffer) = current.as_ref() {
            (buffer.second_index, buffer.second_start)
        } else {
            (0, Duration::from_secs(0))
        }
    }
}

struct SecondBuffer {
    second_index: u64,
    second_start: Duration,
    frames: Vec<BufferedFrame>,
    sample_targets: Vec<f64>,
    next_sample_idx: usize,
    sample_frame_indices: Vec<usize>,
}

impl SecondBuffer {
    fn new(second_index: u64, second_start: Duration, samples_per_second: u32) -> Self {
        let slots = samples_per_second.max(1) as usize;
        let mut sample_targets = Vec::with_capacity(slots);
        for i in 0..slots {
            if i == 0 {
                sample_targets.push(0.0);
            } else {
                sample_targets.push(i as f64 / samples_per_second as f64);
            }
        }

        Self {
            second_index,
            second_start,
            frames: Vec::new(),
            sample_targets,
            next_sample_idx: 0,
            sample_frame_indices: Vec::new(),
        }
    }

    fn push_frame(
        &mut self,
        frame: YPlaneFrame,
        metadata: FrameMetadata,
        detector: &SubtitlePresenceDetector,
        failures: &mut Vec<DetectionFailure>,
    ) {
        let entry = BufferedFrame {
            frame,
            metadata,
            decision: FrameDecision::Pending,
        };
        self.frames.push(entry);
        let frame_idx = self.frames.len() - 1;
        self.assign_samples(frame_idx, detector, failures);
    }

    fn assign_samples(
        &mut self,
        frame_idx: usize,
        detector: &SubtitlePresenceDetector,
        failures: &mut Vec<DetectionFailure>,
    ) {
        if self.sample_targets.is_empty() {
            return;
        }

        let timestamp = self.frames[frame_idx]
            .metadata
            .timestamp
            .or_else(|| self.frames[frame_idx].frame.timestamp())
            .unwrap_or(self.second_start);
        let elapsed = timestamp
            .checked_sub(self.second_start)
            .unwrap_or_else(|| Duration::from_secs(0))
            .as_secs_f64();
        let epsilon = 1e-6f64;

        while self.next_sample_idx < self.sample_targets.len()
            && elapsed + epsilon >= self.sample_targets[self.next_sample_idx]
        {
            self.mark_sample(frame_idx, detector, failures);
        }
    }

    fn mark_sample(
        &mut self,
        frame_idx: usize,
        detector: &SubtitlePresenceDetector,
        failures: &mut Vec<DetectionFailure>,
    ) {
        self.ensure_result(frame_idx, detector, failures);

        if self
            .sample_frame_indices
            .last()
            .copied()
            .unwrap_or(usize::MAX)
            != frame_idx
        {
            self.sample_frame_indices.push(frame_idx);
            if self.sample_frame_indices.len() >= 2 {
                let prev = self.sample_frame_indices[self.sample_frame_indices.len() - 2];
                self.bridge_samples(prev, frame_idx, detector, failures);
            }
        }

        self.next_sample_idx += 1;
    }

    fn bridge_samples(
        &mut self,
        prev_idx: usize,
        curr_idx: usize,
        detector: &SubtitlePresenceDetector,
        failures: &mut Vec<DetectionFailure>,
    ) {
        if curr_idx <= prev_idx {
            return;
        }

        let prev_label = self.frames[prev_idx].decision.has_subtitle();
        let curr_label = self.frames[curr_idx].decision.has_subtitle();

        match (prev_label, curr_label) {
            (Some(label), Some(curr)) if label == curr => {
                for idx in (prev_idx + 1)..curr_idx {
                    if matches!(self.frames[idx].decision, FrameDecision::Pending) {
                        self.frames[idx].decision = FrameDecision::Assumed(label);
                    }
                }
            }
            (Some(prev_label), Some(curr_label)) => {
                self.resolve_transition(
                    prev_idx, curr_idx, prev_label, curr_label, detector, failures,
                );
            }
            _ => {
                self.detect_range(prev_idx + 1, curr_idx, detector, failures);
            }
        }
    }

    fn resolve_transition(
        &mut self,
        prev_idx: usize,
        curr_idx: usize,
        prev_label: bool,
        curr_label: bool,
        detector: &SubtitlePresenceDetector,
        failures: &mut Vec<DetectionFailure>,
    ) {
        if curr_idx <= prev_idx + 1 {
            return;
        }

        let mut left = prev_idx + 1;
        while left < curr_idx {
            self.ensure_result(left, detector, failures);
            match self.frames[left].decision.has_subtitle() {
                Some(label) if label == prev_label => left += 1,
                _ => break,
            }
        }

        if left >= curr_idx {
            return;
        }

        let mut right = curr_idx.saturating_sub(1);
        while right >= left {
            self.ensure_result(right, detector, failures);
            match self.frames[right].decision.has_subtitle() {
                Some(label) if label == curr_label => {
                    if right == 0 {
                        break;
                    }
                    if right == left {
                        break;
                    }
                    right -= 1;
                }
                _ => break,
            }
        }

        if right < left {
            return;
        }

        for idx in left..=right {
            self.ensure_result(idx, detector, failures);
        }
    }

    fn detect_range(
        &mut self,
        start: usize,
        end: usize,
        detector: &SubtitlePresenceDetector,
        failures: &mut Vec<DetectionFailure>,
    ) {
        for idx in start..end {
            self.ensure_result(idx, detector, failures);
        }
    }

    fn ensure_result(
        &mut self,
        idx: usize,
        detector: &SubtitlePresenceDetector,
        failures: &mut Vec<DetectionFailure>,
    ) {
        match self.frames[idx].decision {
            FrameDecision::Evaluated(_) | FrameDecision::Error(_) => return,
            FrameDecision::Assumed(_) | FrameDecision::Pending => {}
        }

        match detector.detect(self.frames[idx].frame.data()) {
            Ok(result) => {
                self.frames[idx].decision = FrameDecision::Evaluated(result);
            }
            Err(err) => {
                let message = err.to_string();
                failures.push(DetectionFailure {
                    metadata: self.frames[idx].metadata.clone(),
                    message: message.clone(),
                });
                self.frames[idx].decision = FrameDecision::Error(message);
            }
        }
    }

    fn finalize_frames(
        &mut self,
        detector: &SubtitlePresenceDetector,
        failures: &mut Vec<DetectionFailure>,
    ) {
        if self.frames.is_empty() {
            return;
        }

        if self.sample_frame_indices.is_empty() {
            self.mark_sample(0, detector, failures);
        }

        while self.next_sample_idx < self.sample_targets.len() {
            let last_idx = self.frames.len() - 1;
            self.mark_sample(last_idx, detector, failures);
        }

        if let Some(&first_sample_idx) = self.sample_frame_indices.first() {
            if let Some(label) = self.frames[first_sample_idx].decision.has_subtitle() {
                for idx in 0..first_sample_idx {
                    if matches!(self.frames[idx].decision, FrameDecision::Pending) {
                        self.frames[idx].decision = FrameDecision::Assumed(label);
                    }
                }
            }
        }

        if let Some(&last_sample_idx) = self.sample_frame_indices.last() {
            if let Some(label) = self.frames[last_sample_idx].decision.has_subtitle() {
                for idx in (last_sample_idx + 1)..self.frames.len() {
                    if matches!(self.frames[idx].decision, FrameDecision::Pending) {
                        self.frames[idx].decision = FrameDecision::Assumed(label);
                    }
                }
            }
        }

        for idx in 0..self.frames.len() {
            if matches!(self.frames[idx].decision, FrameDecision::Pending) {
                self.ensure_result(idx, detector, failures);
            }
        }
    }

    fn into_frames(
        &mut self,
        detector: &SubtitlePresenceDetector,
        failures: &mut Vec<DetectionFailure>,
    ) -> Vec<BufferedFrame> {
        self.finalize_frames(detector, failures);
        std::mem::take(&mut self.frames)
    }
}

struct DetectionDebug {
    path: PathBuf,
    entries: Vec<DetectionDebugEntry>,
}

impl DetectionDebug {
    fn record_success(&mut self, metadata: &FrameMetadata, result: &SubtitleDetectionResult) {
        self.entries
            .push(DetectionDebugEntry::from_result(metadata, result));
    }

    fn record_assumption(&mut self, metadata: &FrameMetadata, has_subtitle: bool) {
        self.entries
            .push(DetectionDebugEntry::from_assumption(metadata, has_subtitle));
    }

    fn record_error(&mut self, metadata: &FrameMetadata, error: String) {
        self.entries
            .push(DetectionDebugEntry::from_error(metadata, error));
    }

    fn write_json(self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let json = serde_json::to_string_pretty(&self.entries)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))?;
        std::fs::write(&self.path, json)
    }
}

#[derive(Serialize, Clone)]
struct DetectionDebugEntry {
    frame_index: u64,
    processed_index: u64,
    timestamp_seconds: Option<f64>,
    edge_energy_ratio: Option<f32>,
    run_ratio: Option<f32>,
    dt_coefficient_of_variation: Option<f32>,
    cc_density: Option<f32>,
    banner_score: Option<f32>,
    score: Option<f32>,
    has_subtitle: Option<bool>,
    error: Option<String>,
    assumed: Option<bool>,
}

impl DetectionDebugEntry {
    fn from_result(metadata: &FrameMetadata, result: &SubtitleDetectionResult) -> Self {
        Self {
            frame_index: metadata.frame_index,
            processed_index: metadata.processed_index,
            timestamp_seconds: metadata.timestamp.map(|ts| ts.as_secs_f64()),
            edge_energy_ratio: Some(result.edge_energy_ratio),
            run_ratio: Some(result.run_ratio),
            dt_coefficient_of_variation: Some(result.dt_coefficient_of_variation),
            cc_density: Some(result.cc_density),
            banner_score: Some(result.banner_score),
            score: Some(result.score),
            has_subtitle: Some(result.has_subtitle),
            error: None,
            assumed: None,
        }
    }

    fn from_error(metadata: &FrameMetadata, error: String) -> Self {
        Self {
            frame_index: metadata.frame_index,
            processed_index: metadata.processed_index,
            timestamp_seconds: metadata.timestamp.map(|ts| ts.as_secs_f64()),
            edge_energy_ratio: None,
            run_ratio: None,
            dt_coefficient_of_variation: None,
            cc_density: None,
            banner_score: None,
            score: None,
            has_subtitle: None,
            error: Some(error),
            assumed: None,
        }
    }

    fn from_assumption(metadata: &FrameMetadata, has_subtitle: bool) -> Self {
        Self {
            frame_index: metadata.frame_index,
            processed_index: metadata.processed_index,
            timestamp_seconds: metadata.timestamp.map(|ts| ts.as_secs_f64()),
            edge_energy_ratio: None,
            run_ratio: None,
            dt_coefficient_of_variation: None,
            cc_density: None,
            banner_score: None,
            score: None,
            has_subtitle: Some(has_subtitle),
            error: None,
            assumed: Some(true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::Duration;

    fn temp_debug_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("subtitle_fast_{name}.json"))
    }

    fn build_state(samples_per_second: u32, path: PathBuf) -> SubtitleDetectionState {
        let options = SubtitleDetectionOptions {
            enabled: true,
            debug_output: Some(path),
            samples_per_second,
        };
        SubtitleDetectionState::new(options)
    }

    fn make_subtitle_plane(width: usize, height: usize) -> Vec<u8> {
        let stride = width;
        let mut data = vec![24u8; stride * height];
        let roi_height = (height as f32 * 0.25) as usize;
        let start_row = height.saturating_sub(roi_height.max(1));
        for y in start_row..height {
            for x in (width / 8)..(width - width / 8) {
                let stripe = ((y - start_row) / 2) % 2;
                data[y * stride + x] = if stripe == 0 { 235 } else { 16 };
            }
        }
        data
    }

    fn make_blank_plane(width: usize, height: usize) -> Vec<u8> {
        vec![32u8; width * height]
    }

    fn make_frame(
        plane: Vec<u8>,
        width: usize,
        height: usize,
        timestamp: f64,
        index: u64,
    ) -> (YPlaneFrame, FrameMetadata) {
        let stride = width;
        let duration = Some(Duration::from_secs_f64(timestamp));
        let frame = YPlaneFrame::from_owned(width as u32, height as u32, stride, duration, plane)
            .unwrap()
            .with_frame_index(Some(index));
        let metadata = FrameMetadata {
            frame_index: index,
            processed_index: index,
            timestamp: duration,
        };
        (frame, metadata)
    }

    #[test]
    fn sampling_assumes_between_matching_samples() {
        let path = temp_debug_path("assume");
        let mut state = build_state(2, path.clone());
        let width = 256usize;
        let height = 144usize;

        let subtitle_plane = make_subtitle_plane(width, height);
        let detector =
            SubtitlePresenceDetector::new(SubtitleDetectionConfig::for_frame(width, height, width))
                .unwrap();
        assert!(detector.detect(&subtitle_plane).unwrap().has_subtitle);

        let (frame0, meta0) = make_frame(subtitle_plane.clone(), width, height, 0.0, 0);
        let (frame1, meta1) = make_frame(subtitle_plane.clone(), width, height, 0.25, 1);
        let (frame2, meta2) = make_frame(subtitle_plane.clone(), width, height, 0.6, 2);
        let (frame3, meta3) = make_frame(make_blank_plane(width, height), width, height, 1.0, 3);

        state.process_frame(&frame0, &meta0);
        state.process_frame(&frame1, &meta1);
        state.process_frame(&frame2, &meta2);
        state.process_frame(&frame3, &meta3);

        let entries = state
            .debug
            .as_ref()
            .expect("debug entries available")
            .entries
            .clone();
        let assumed = entries
            .iter()
            .find(|entry| entry.frame_index == 1)
            .expect("frame 1 entry");
        assert_eq!(assumed.has_subtitle, Some(true));
        assert_eq!(assumed.assumed, Some(true));

        state.finalize().unwrap();
        let _ = fs::remove_file(path);
    }

    #[test]
    fn sampling_detects_transition_between_samples() {
        let path = temp_debug_path("transition");
        let mut state = build_state(2, path.clone());
        let width = 256usize;
        let height = 144usize;

        let subtitle_plane = make_subtitle_plane(width, height);
        let blank_plane = make_blank_plane(width, height);

        let detector =
            SubtitlePresenceDetector::new(SubtitleDetectionConfig::for_frame(width, height, width))
                .unwrap();
        assert!(detector.detect(&subtitle_plane).unwrap().has_subtitle);
        assert!(!detector.detect(&blank_plane).unwrap().has_subtitle);

        let (frame0, meta0) = make_frame(subtitle_plane.clone(), width, height, 0.0, 0);
        let (frame1, meta1) = make_frame(blank_plane.clone(), width, height, 0.3, 1);
        let (frame2, meta2) = make_frame(blank_plane.clone(), width, height, 0.6, 2);
        let (frame3, meta3) = make_frame(subtitle_plane.clone(), width, height, 1.0, 3);

        state.process_frame(&frame0, &meta0);
        state.process_frame(&frame1, &meta1);
        state.process_frame(&frame2, &meta2);
        state.process_frame(&frame3, &meta3);

        let entries = state
            .debug
            .as_ref()
            .expect("debug entries available")
            .entries
            .clone();
        let transition = entries
            .iter()
            .find(|entry| entry.frame_index == 1)
            .expect("frame 1 entry");
        assert_eq!(transition.has_subtitle, Some(false));
        assert!(transition.assumed.is_none());

        state.finalize().unwrap();
        let _ = fs::remove_file(path);
    }
}

async fn write_frame(
    frame: &YPlaneFrame,
    index: u64,
    directory: &Path,
    quality: u8,
) -> Result<(), WriteFrameError> {
    let width = frame.width() as usize;
    let height = frame.height() as usize;
    if width == 0 || height == 0 {
        return Ok(());
    }
    let stride = frame.stride();
    let required = stride
        .checked_mul(height)
        .ok_or(WriteFrameError::PlaneBounds {
            stride,
            width,
            height,
        })?;
    let data = frame.data();
    if data.len() < required {
        return Err(WriteFrameError::PlaneBounds {
            stride,
            width,
            height,
        });
    }

    let mut buffer = vec![0u8; width * height];
    for (row_idx, dest_row) in buffer.chunks_mut(width).enumerate() {
        let start = row_idx * stride;
        let end = start + width;
        dest_row.copy_from_slice(&data[start..end]);
    }

    let mut encoded = Vec::new();
    {
        let mut encoder = JpegEncoder::new_with_quality(&mut encoded, quality);
        encoder.encode(&buffer, frame.width(), frame.height(), ColorType::L8)?;
    }

    let filename = format!("frame_{index}.jpg");
    let path = directory.join(filename);
    task::spawn_blocking(move || std::fs::write(path, encoded))
        .await
        .map_err(|err| {
            WriteFrameError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("join error: {err}"),
            ))
        })??;
    Ok(())
}

#[derive(Debug, Error)]
enum WriteFrameError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("encoding error: {0}")]
    Encode(#[from] image::ImageError),
    #[error("invalid plane dimensions stride={stride} width={width} height={height}")]
    PlaneBounds {
        stride: usize,
        width: usize,
        height: usize,
    },
}
