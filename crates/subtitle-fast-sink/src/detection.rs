use std::time::Duration;

use crate::config::{FrameMetadata, SubtitleDetectionOptions};
use crate::subtitle_detection::{
    build_detector, SubtitleDetectionConfig, SubtitleDetectionResult, SubtitleDetector,
};
use subtitle_fast_decoder::YPlaneFrame;
use tokio::sync::Mutex;

pub(crate) struct SubtitleDetectionOperation {
    state: Mutex<SubtitleDetectionState>,
}

impl SubtitleDetectionOperation {
    pub fn new(options: SubtitleDetectionOptions) -> Self {
        Self {
            state: Mutex::new(SubtitleDetectionState::new(options)),
        }
    }

    pub async fn process(&self, frame: &YPlaneFrame, metadata: &FrameMetadata) {
        let mut state = self.state.lock().await;
        state.process_frame(frame, metadata);
    }

    pub async fn finalize(&self) {
        let mut state = self.state.lock().await;
        let _ = state.finalize();
    }
}

struct SubtitleDetectionState {
    detector: Option<Box<dyn SubtitleDetector>>,
    detector_dims: Option<(usize, usize, usize)>,
    init_error_logged: bool,
    sampling: SubtitleSamplingState,
    options: SubtitleDetectionOptions,
}

impl SubtitleDetectionState {
    fn new(options: SubtitleDetectionOptions) -> Self {
        let samples_per_second = options.samples_per_second.max(1);
        Self {
            detector: None,
            detector_dims: None,
            init_error_logged: false,
            sampling: SubtitleSamplingState::new(samples_per_second),
            options,
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
            let mut detector_config = SubtitleDetectionConfig::for_frame(dims.0, dims.1, dims.2);
            detector_config.dump_json = self.options.dump_json;
            detector_config.model_path = self.options.onnx_model_path.clone();
            if let Some(roi) = self.options.roi_override {
                detector_config.roi = roi;
            }
            match build_detector(self.options.detector, detector_config) {
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
            .ingest_frame(frame_clone, metadata_clone, detector.as_ref());
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
            let output = self.sampling.finalize(detector.as_ref());
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
        detector: &dyn SubtitleDetector,
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

    fn finalize(&mut self, detector: &dyn SubtitleDetector) -> SamplingOutput {
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
        detector: &dyn SubtitleDetector,
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
        detector: &dyn SubtitleDetector,
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
        detector: &dyn SubtitleDetector,
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
        detector: &dyn SubtitleDetector,
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
        detector: &dyn SubtitleDetector,
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
        detector: &dyn SubtitleDetector,
        failures: &mut Vec<DetectionFailure>,
    ) {
        for idx in start..end {
            self.ensure_result(idx, detector, failures);
        }
    }

    fn ensure_result(
        &mut self,
        idx: usize,
        detector: &dyn SubtitleDetector,
        failures: &mut Vec<DetectionFailure>,
    ) {
        match self.frames[idx].decision {
            FrameDecision::Evaluated(_) | FrameDecision::Error => return,
            FrameDecision::Assumed(_) | FrameDecision::Pending => {}
        }

        match detector.detect(self.frames[idx].frame.data(), &self.frames[idx].metadata) {
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
        detector: &dyn SubtitleDetector,
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
        detector: &dyn SubtitleDetector,
        failures: &mut Vec<DetectionFailure>,
    ) -> Vec<BufferedFrame> {
        self.finalize_frames(detector, failures);
        std::mem::take(&mut self.frames)
    }
}
