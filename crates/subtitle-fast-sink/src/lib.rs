use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::{ColorType, ImageEncoder};
use subtitle_fast_decoder::YPlaneFrame;
use thiserror::Error;
use tokio::sync::{mpsc, Mutex, Semaphore};
use tokio::task::{self, JoinSet};

pub mod subtitle_detection;

use subtitle_detection::{
    SubtitleDetectionConfig, SubtitleDetectionResult, SubtitlePresenceDetector,
};

const DEFAULT_CHANNEL_CAPACITY: usize = 64;
const DEFAULT_MAX_CONCURRENCY: usize = 16;

#[derive(Clone, Debug)]
pub struct FrameSinkConfig {
    pub channel_capacity: usize,
    pub max_concurrency: usize,
    pub dump: Option<FrameDumpConfig>,
    pub detection: SubtitleDetectionOptions,
    pub progress_callback: Option<FrameSinkProgressCallback>,
}

impl Default for FrameSinkConfig {
    fn default() -> Self {
        Self {
            channel_capacity: DEFAULT_CHANNEL_CAPACITY,
            max_concurrency: DEFAULT_MAX_CONCURRENCY,
            dump: None,
            detection: SubtitleDetectionOptions::default(),
            progress_callback: None,
        }
    }
}

impl FrameSinkConfig {
    pub fn from_outputs(
        dump_dir: Option<PathBuf>,
        format: ImageOutputFormat,
        samples_per_second: u32,
    ) -> Self {
        let mut config = Self::default();
        if let Some(dir) = dump_dir {
            config.dump = Some(FrameDumpConfig::new(dir, format, samples_per_second));
        }
        config.detection.samples_per_second = samples_per_second.max(1);
        config
    }

    pub fn with_progress_callback(mut self, callback: FrameSinkProgressCallback) -> Self {
        self.progress_callback = Some(callback);
        self
    }

    pub fn set_progress_callback(&mut self, callback: FrameSinkProgressCallback) {
        self.progress_callback = Some(callback);
    }
}

#[derive(Clone, Debug)]
pub struct FrameDumpConfig {
    pub directory: PathBuf,
    pub format: ImageOutputFormat,
    pub samples_per_second: u32,
}

impl FrameDumpConfig {
    pub fn new(directory: PathBuf, format: ImageOutputFormat, samples_per_second: u32) -> Self {
        Self {
            directory,
            format,
            samples_per_second: samples_per_second.max(1),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub enum ImageOutputFormat {
    Jpeg { quality: u8 },
    Png,
    Webp,
}

#[derive(Clone, Debug)]
pub struct SubtitleDetectionOptions {
    pub enabled: bool,
    pub samples_per_second: u32,
}

impl Default for SubtitleDetectionOptions {
    fn default() -> Self {
        Self {
            enabled: true,
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

#[derive(Clone, Debug)]
pub struct FrameSinkProgressEvent {
    pub completed: u64,
    pub metadata: FrameMetadata,
}

#[derive(Clone)]
pub struct FrameSinkProgressCallback {
    inner: Arc<dyn Fn(FrameSinkProgressEvent) + Send + Sync>,
}

impl FrameSinkProgressCallback {
    pub fn new<F>(callback: F) -> Self
    where
        F: Fn(FrameSinkProgressEvent) + Send + Sync + 'static,
    {
        Self {
            inner: Arc::new(callback),
        }
    }

    pub fn from_arc(callback: Arc<dyn Fn(FrameSinkProgressEvent) + Send + Sync>) -> Self {
        Self { inner: callback }
    }

    pub fn call(&self, event: FrameSinkProgressEvent) {
        (self.inner)(event);
    }
}

impl fmt::Debug for FrameSinkProgressCallback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FrameSinkProgressCallback").finish()
    }
}

impl<F> From<F> for FrameSinkProgressCallback
where
    F: Fn(FrameSinkProgressEvent) + Send + Sync + 'static,
{
    fn from(callback: F) -> Self {
        Self::new(callback)
    }
}

impl From<Arc<dyn Fn(FrameSinkProgressEvent) + Send + Sync>> for FrameSinkProgressCallback {
    fn from(callback: Arc<dyn Fn(FrameSinkProgressEvent) + Send + Sync>) -> Self {
        Self::from_arc(callback)
    }
}

pub struct FrameSink {
    sender: mpsc::Sender<Job>,
    worker: tokio::task::JoinHandle<()>,
}

impl FrameSink {
    pub fn new(config: FrameSinkConfig) -> Self {
        let capacity = config.channel_capacity.max(1);
        let concurrency = config.max_concurrency.max(1);
        let operations = Arc::new(ProcessingOperations::new(config));
        let (sender, mut rx) = mpsc::channel::<Job>(capacity);
        let semaphore = Arc::new(Semaphore::new(concurrency));
        let worker_operations = Arc::clone(&operations);
        let worker_semaphore = Arc::clone(&semaphore);

        let worker = tokio::spawn(async move {
            let operations = worker_operations;
            let semaphore = worker_semaphore;
            let mut tasks = JoinSet::new();

            while let Some(job) = rx.recv().await {
                let permit = match semaphore.clone().acquire_owned().await {
                    Ok(permit) => permit,
                    Err(err) => {
                        eprintln!("frame sink semaphore error: {err}");
                        break;
                    }
                };

                let operations = Arc::clone(&operations);
                tasks.spawn(async move {
                    let Job { frame, metadata } = job;
                    let _permit = permit;
                    operations.process_frame(frame, metadata).await;
                });
            }

            while let Some(result) = tasks.join_next().await {
                if let Err(err) = result {
                    if !err.is_cancelled() {
                        eprintln!("frame sink join error: {err}");
                    }
                }
            }

            operations.finalize().await;
        });

        Self { sender, worker }
    }

    pub async fn push(&self, frame: YPlaneFrame, metadata: FrameMetadata) -> bool {
        self.sender.send(Job { frame, metadata }).await.is_ok()
    }

    pub async fn shutdown(self) {
        drop(self.sender);
        if let Err(err) = self.worker.await {
            if !err.is_cancelled() {
                eprintln!("frame sink worker task error: {err}");
            }
        }
    }
}

struct Job {
    frame: YPlaneFrame,
    metadata: FrameMetadata,
}

struct ProcessingOperations {
    dump: Option<Arc<FrameDumpOperation>>,
    detection: Option<Arc<SubtitleDetectionOperation>>,
    progress: Option<FrameSinkProgressReporter>,
}

impl ProcessingOperations {
    fn new(config: FrameSinkConfig) -> Self {
        let dump = config
            .dump
            .map(|cfg| Arc::new(FrameDumpOperation::new(cfg)));
        let detection = if config.detection.enabled {
            Some(Arc::new(SubtitleDetectionOperation::new(config.detection)))
        } else {
            None
        };
        let progress = config.progress_callback.map(FrameSinkProgressReporter::new);
        Self {
            dump,
            detection,
            progress,
        }
    }

    async fn process_frame(&self, frame: YPlaneFrame, metadata: FrameMetadata) {
        if let Some(dump) = self.dump.as_ref() {
            if let Err(err) = dump.process(&frame, &metadata).await {
                eprintln!("frame sink dump error: {err}");
            }
        }

        if let Some(detection) = self.detection.as_ref() {
            detection.process(&frame, &metadata).await;
        }

        if let Some(progress) = self.progress.as_ref() {
            progress.frame_completed(&metadata);
        }
    }

    async fn finalize(&self) {
        if let Some(dump) = self.dump.as_ref() {
            if let Err(err) = dump.finalize().await {
                eprintln!("frame sink dump finalize error: {err}");
            }
        }

        if let Some(detection) = self.detection.as_ref() {
            detection.finalize().await;
        }
    }
}

#[derive(Clone)]
struct FrameSinkProgressReporter {
    callback: FrameSinkProgressCallback,
    completed: Arc<AtomicU64>,
}

impl FrameSinkProgressReporter {
    fn new(callback: FrameSinkProgressCallback) -> Self {
        Self {
            callback,
            completed: Arc::new(AtomicU64::new(0)),
        }
    }

    fn frame_completed(&self, metadata: &FrameMetadata) {
        let completed = self.completed.fetch_add(1, Ordering::SeqCst) + 1;
        self.callback.call(FrameSinkProgressEvent {
            completed,
            metadata: metadata.clone(),
        });
    }
}

struct FrameDumpOperation {
    directory: Arc<PathBuf>,
    format: ImageOutputFormat,
    state: Mutex<FrameDumpState>,
}

impl FrameDumpOperation {
    fn new(config: FrameDumpConfig) -> Self {
        Self {
            directory: Arc::new(config.directory),
            format: config.format,
            state: Mutex::new(FrameDumpState::new(config.samples_per_second)),
        }
    }

    async fn process(
        &self,
        frame: &YPlaneFrame,
        metadata: &FrameMetadata,
    ) -> Result<(), WriteFrameError> {
        let ready = {
            let mut state = self.state.lock().await;
            state.enqueue_frame(frame.clone(), metadata.clone())
        };

        self.write_batch(ready).await
    }

    async fn finalize(&self) -> Result<(), WriteFrameError> {
        let remaining = {
            let mut state = self.state.lock().await;
            state.drain_pending()
        };

        self.write_batch(remaining).await
    }

    async fn write_batch(&self, frames: Vec<QueuedDumpFrame>) -> Result<(), WriteFrameError> {
        for frame in frames {
            write_frame(
                &frame.frame,
                frame.metadata.frame_index,
                &self.directory,
                self.format,
            )
            .await?;
        }
        Ok(())
    }
}

struct FrameDumpState {
    sampler: FrameDumpSampler,
    pending: BTreeMap<u64, QueuedDumpFrame>,
    next_processed_index: u64,
}

impl FrameDumpState {
    fn new(samples_per_second: u32) -> Self {
        Self {
            sampler: FrameDumpSampler::new(samples_per_second),
            pending: BTreeMap::new(),
            next_processed_index: 1,
        }
    }

    fn enqueue_frame(
        &mut self,
        frame: YPlaneFrame,
        metadata: FrameMetadata,
    ) -> Vec<QueuedDumpFrame> {
        let index = metadata.processed_index;
        self.pending
            .insert(index, QueuedDumpFrame { frame, metadata });
        self.collect_ready()
    }

    fn drain_pending(&mut self) -> Vec<QueuedDumpFrame> {
        let mut drained = Vec::new();
        while !self.pending.is_empty() {
            let ready = self.collect_ready();
            if ready.is_empty() {
                if let Some(&next_index) = self.pending.keys().next() {
                    self.next_processed_index = next_index;
                }
            } else {
                drained.extend(ready);
            }
        }
        drained
    }

    fn collect_ready(&mut self) -> Vec<QueuedDumpFrame> {
        let mut ready = Vec::new();
        loop {
            let index = self.next_processed_index;
            match self.pending.remove(&index) {
                Some(frame) => {
                    self.next_processed_index = self.next_processed_index.saturating_add(1);
                    if self.sampler.should_write(&frame.frame, &frame.metadata) {
                        ready.push(frame);
                    }
                }
                None => break,
            }
        }
        ready
    }
}

struct QueuedDumpFrame {
    frame: YPlaneFrame,
    metadata: FrameMetadata,
}

struct FrameDumpSampler {
    samples_per_second: u32,
    current: Option<SamplerSecond>,
}

impl FrameDumpSampler {
    fn new(samples_per_second: u32) -> Self {
        Self {
            samples_per_second: samples_per_second.max(1),
            current: None,
        }
    }

    fn should_write(&mut self, frame: &YPlaneFrame, metadata: &FrameMetadata) -> bool {
        let (second_index, elapsed) = self.resolve_second(frame, metadata);

        if self.current.as_ref().map(|second| second.index) != Some(second_index) {
            self.current = Some(SamplerSecond::new(second_index, self.samples_per_second));
        }

        let Some(current) = self.current.as_mut() else {
            return false;
        };

        current.consume(elapsed)
    }

    fn resolve_second(&self, frame: &YPlaneFrame, metadata: &FrameMetadata) -> (u64, f64) {
        if let Some(timestamp) = metadata.timestamp.or_else(|| frame.timestamp()) {
            let second_index = timestamp.as_secs();
            let elapsed = timestamp
                .checked_sub(Duration::from_secs(second_index))
                .unwrap_or_else(|| Duration::from_secs(0))
                .as_secs_f64();
            return (second_index, elapsed);
        }

        let samples = self.samples_per_second.max(1) as u64;
        let processed = metadata.processed_index.saturating_sub(1);
        let second_index = processed / samples;
        let offset = processed.saturating_sub(second_index * samples);
        let elapsed = offset as f64 / self.samples_per_second.max(1) as f64;
        (second_index, elapsed)
    }
}

struct SamplerSecond {
    index: u64,
    targets: Vec<f64>,
    next_target_idx: usize,
}

impl SamplerSecond {
    fn new(index: u64, samples_per_second: u32) -> Self {
        let slots = samples_per_second.max(1) as usize;
        let mut targets = Vec::with_capacity(slots);
        for i in 0..slots {
            if i == 0 {
                targets.push(0.0);
            } else {
                targets.push(i as f64 / samples_per_second as f64);
            }
        }
        Self {
            index,
            targets,
            next_target_idx: 0,
        }
    }

    fn consume(&mut self, elapsed: f64) -> bool {
        if self.targets.is_empty() {
            return false;
        }

        let mut should_write = false;
        let epsilon = 1e-6f64;

        while self.next_target_idx < self.targets.len()
            && elapsed + epsilon >= self.targets[self.next_target_idx]
        {
            should_write = true;
            self.next_target_idx += 1;
        }

        should_write
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
        let _ = state.finalize();
    }
}

struct SubtitleDetectionState {
    detector: Option<SubtitlePresenceDetector>,
    detector_dims: Option<(usize, usize, usize)>,
    init_error_logged: bool,
    sampling: SubtitleSamplingState,
}

impl SubtitleDetectionState {
    fn new(options: SubtitleDetectionOptions) -> Self {
        Self {
            detector: None,
            detector_dims: None,
            init_error_logged: false,
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
                    return;
                }
            }
        }

        let Some(detector) = self.detector.as_ref() else {
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
    }

    fn finalize(&mut self) -> std::io::Result<()> {
        if let Some(detector) = self.detector.as_ref() {
            let output = self.sampling.finalize(detector);
            self.handle_sampling_output(output);
        } else {
            self.sampling.reset();
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
    Error,
}

impl FrameDecision {
    fn has_subtitle(&self) -> Option<bool> {
        match self {
            FrameDecision::Evaluated(result) => Some(result.has_subtitle),
            FrameDecision::Assumed(value) => Some(*value),
            FrameDecision::Error | FrameDecision::Pending => None,
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
            FrameDecision::Evaluated(_) | FrameDecision::Error => return,
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
                self.frames[idx].decision = FrameDecision::Error;
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

async fn write_frame(
    frame: &YPlaneFrame,
    index: u64,
    directory: &Path,
    format: ImageOutputFormat,
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
    let extension = match format {
        ImageOutputFormat::Jpeg { quality } => {
            let mut encoder = JpegEncoder::new_with_quality(&mut encoded, quality);
            encoder.encode(&buffer, frame.width(), frame.height(), ColorType::L8)?;
            "jpg"
        }
        ImageOutputFormat::Png => {
            let encoder = PngEncoder::new(&mut encoded);
            encoder.write_image(&buffer, frame.width(), frame.height(), ColorType::L8)?;
            "png"
        }
        ImageOutputFormat::Webp => {
            let encoder = WebPEncoder::new_lossless(&mut encoded);
            encoder.encode(&buffer, frame.width(), frame.height(), ColorType::L8)?;
            "webp"
        }
    };

    let filename = format!("frame_{index}.{extension}");
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
