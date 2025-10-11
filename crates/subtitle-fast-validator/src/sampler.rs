use std::collections::BTreeMap;

use crate::config::FrameMetadata;
use subtitle_fast_decoder::YPlaneFrame;

#[derive(Clone)]
pub struct SampledFrame {
    pub frame: YPlaneFrame,
    pub metadata: FrameMetadata,
}

pub struct FrameSampleCoordinator {
    sampler: FrameSampler,
    pending: BTreeMap<u64, SampledFrame>,
    next_processed_index: u64,
}

impl FrameSampleCoordinator {
    pub fn new(samples_per_second: u32) -> Self {
        Self {
            sampler: FrameSampler::new(samples_per_second),
            pending: BTreeMap::new(),
            next_processed_index: 1,
        }
    }

    pub fn enqueue(&mut self, frame: YPlaneFrame, metadata: FrameMetadata) -> Vec<SampledFrame> {
        let index = metadata.processed_index;
        self.pending.insert(index, SampledFrame { frame, metadata });
        self.collect_ready()
    }

    pub fn drain(&mut self) -> Vec<SampledFrame> {
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

    fn collect_ready(&mut self) -> Vec<SampledFrame> {
        let mut ready = Vec::new();
        loop {
            let index = self.next_processed_index;
            match self.pending.remove(&index) {
                Some(frame) => {
                    self.next_processed_index = self.next_processed_index.saturating_add(1);
                    if self.sampler.should_sample(&frame.frame, &frame.metadata) {
                        ready.push(frame);
                    }
                }
                None => break,
            }
        }
        ready
    }
}

struct FrameSampler {
    samples_per_second: u32,
    current: Option<SamplerSecond>,
}

impl FrameSampler {
    fn new(samples_per_second: u32) -> Self {
        Self {
            samples_per_second: samples_per_second.max(1),
            current: None,
        }
    }

    fn should_sample(&mut self, frame: &YPlaneFrame, metadata: &FrameMetadata) -> bool {
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
        use std::time::Duration;

        if let Some(timestamp) = metadata.timestamp.or_else(|| frame.timestamp()) {
            let second_index = timestamp.as_secs();
            let elapsed = timestamp
                .checked_sub(std::time::Duration::from_secs(second_index))
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
