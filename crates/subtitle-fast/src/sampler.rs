use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{StreamExt, stream::unfold};
use tokio::sync::mpsc;

use crate::stage::{PipelineStage, StageInput, StageOutput};
use subtitle_fast_decoder::{YPlaneError, YPlaneFrame, YPlaneResult};

const SAMPLER_CHANNEL_CAPACITY: usize = 1;
const DEFAULT_POOL_CAPACITY: usize = 24;
const MAX_POOL_CAPACITY: usize = 240;
const EPSILON: f64 = 1e-6;

pub type SamplerResult = Result<SampledFrame, YPlaneError>;

#[derive(Debug, Clone, Copy)]
pub enum FrameType {
    Sampled,
    Skipped,
}

pub struct SampledFrame {
    pub frame_index: u64,
    pub frame_type: FrameType,
    frame: YPlaneFrame,
    history: FrameHistory,
    completion: Option<mpsc::Sender<PoolEntry>>,
}

impl SampledFrame {
    fn new(
        frame_index: u64,
        frame_type: FrameType,
        frame: YPlaneFrame,
        history: FrameHistory,
        completion: mpsc::Sender<PoolEntry>,
    ) -> Self {
        Self {
            frame_index,
            frame_type,
            frame,
            history,
            completion: Some(completion),
        }
    }

    #[allow(dead_code)]
    pub fn frame(&self) -> &YPlaneFrame {
        &self.frame
    }

    #[allow(dead_code)]
    pub fn into_frame(self) -> YPlaneFrame {
        self.frame
    }

    #[allow(dead_code)]
    pub fn history(&self) -> &FrameHistory {
        &self.history
    }

    pub async fn finish(self) {
        let SampledFrame {
            frame_index,
            frame_type,
            frame,
            history: _history,
            completion,
        } = self;

        if let Some(tx) = completion {
            let entry = PoolEntry::new(frame_index, frame_type, Arc::new(frame));
            let _ = tx.send(entry).await;
        }
    }
}

pub struct FrameSampler {
    samples_per_second: u32,
}

impl FrameSampler {
    pub fn new(samples_per_second: u32) -> Self {
        Self {
            samples_per_second,
        }
    }
}

impl PipelineStage<YPlaneResult<YPlaneFrame>> for FrameSampler {
    type Output = SamplerResult;

    fn name(&self) -> &'static str {
        "frame_sampler"
    }

    fn apply(
        self: Box<Self>,
        input: StageInput<YPlaneResult<YPlaneFrame>>,
    ) -> StageOutput<Self::Output> {
        let StageInput {
            stream,
            total_frames,
        } = input;

        let samples_per_second = self.samples_per_second;
        let (tx, rx) = mpsc::channel::<SamplerResult>(SAMPLER_CHANNEL_CAPACITY);
        let (completion_tx, mut completion_rx) = mpsc::channel::<PoolEntry>(MAX_POOL_CAPACITY);

        tokio::spawn(async move {
            let mut upstream = stream;
            let mut worker = SamplerWorker::new(samples_per_second, completion_tx);

            loop {
                tokio::select! {
                    Some(entry) = completion_rx.recv() => {
                        worker.reclaim(entry);
                        continue;
                    }
                    maybe_item = upstream.next() => {
                        match maybe_item {
                            Some(Ok(frame)) => {
                                if worker.handle_frame(frame, &tx).await.is_err() {
                                    break;
                                }
                            }
                            Some(Err(err)) => {
                                let _ = tx.send(Err(err)).await;
                                break;
                            }
                            None => break,
                        }
                    }
                }
            }

            while let Some(entry) = completion_rx.recv().await {
                worker.reclaim(entry);
            }
        });

        let stream = Box::pin(unfold(rx, |mut receiver| async {
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

struct SamplerWorker {
    processed: u64,
    pool: SamplerPool,
    schedule: SampleSchedule,
    fps: FpsEstimator,
    completion_tx: mpsc::Sender<PoolEntry>,
}

impl SamplerWorker {
    fn new(samples_per_second: u32, completion_tx: mpsc::Sender<PoolEntry>) -> Self {
        Self {
            processed: 0,
            pool: SamplerPool::new(DEFAULT_POOL_CAPACITY),
            schedule: SampleSchedule::new(samples_per_second),
            fps: FpsEstimator::new(),
            completion_tx,
        }
    }

    async fn handle_frame(
        &mut self,
        frame: YPlaneFrame,
        tx: &mpsc::Sender<SamplerResult>,
    ) -> Result<(), ()> {
        self.processed = self.processed.saturating_add(1);
        let processed_index = self.processed;

        let frame_index = frame
            .frame_index()
            .unwrap_or_else(|| processed_index.saturating_sub(1));
        let timestamp = frame.timestamp();

        if let Some(capacity) = self.fps.observe(frame_index, timestamp) {
            self.pool.set_capacity(capacity);
        }

        let frame_type = if self.schedule.should_sample(timestamp, processed_index) {
            FrameType::Sampled
        } else {
            FrameType::Skipped
        };

        match frame_type {
            FrameType::Skipped => {
                let frame_arc = Arc::new(frame);
                self.pool
                    .push(PoolEntry::new(frame_index, frame_type, frame_arc));
            }
            FrameType::Sampled => {
                let history = self.pool.snapshot();
                let sample = SampledFrame::new(
                    frame_index,
                    frame_type,
                    frame,
                    history,
                    self.completion_tx.clone(),
                );
                if tx.send(Ok(sample)).await.is_err() {
                    return Err(());
                }
            }
        }

        println!("sampler pool size: {}", self.pool.len());

        Ok(())
    }

    fn reclaim(&mut self, entry: PoolEntry) {
        self.pool.push(entry);
    }
}

struct SamplerPool {
    entries: VecDeque<PoolEntry>,
    capacity: usize,
}

impl SamplerPool {
    fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    fn set_capacity(&mut self, capacity: usize) {
        let bounded = capacity.clamp(1, MAX_POOL_CAPACITY);
        self.capacity = bounded;
        self.trim();
    }

    fn push(&mut self, entry: PoolEntry) {
        self.entries.push_back(entry);
        self.trim();
    }

    fn snapshot(&self) -> FrameHistory {
        let mut records = Vec::with_capacity(self.entries.len());
        for entry in self.entries.iter() {
            records.push(HistoryRecord {
                frame_index: entry.frame_index,
                frame_type: entry.frame_type,
                frame: Arc::clone(&entry.frame),
            });
        }
        FrameHistory::new(records)
    }

    fn trim(&mut self) {
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

struct SampleSchedule {
    samples_per_second: u32,
    current_second: Option<u64>,
    targets: Vec<f64>,
    next_target_idx: usize,
}

impl SampleSchedule {
    fn new(samples_per_second: u32) -> Self {
        let samples = samples_per_second;
        let mut targets = Vec::with_capacity(samples as usize);
        for i in 0..samples {
            let target = if i == 0 {
                0.0
            } else {
                i as f64 / samples as f64
            };
            targets.push(target);
        }

        Self {
            samples_per_second: samples,
            current_second: None,
            targets,
            next_target_idx: 0,
        }
    }

    fn should_sample(&mut self, timestamp: Option<Duration>, processed_index: u64) -> bool {
        let (second_index, elapsed) = self.resolve_second(timestamp, processed_index);

        if self.current_second != Some(second_index) {
            self.current_second = Some(second_index);
            self.next_target_idx = 0;
        }

        let mut should_sample = false;
        while self.next_target_idx < self.targets.len()
            && elapsed + EPSILON >= self.targets[self.next_target_idx]
        {
            should_sample = true;
            self.next_target_idx += 1;
        }

        should_sample
    }

    fn resolve_second(&self, timestamp: Option<Duration>, processed_index: u64) -> (u64, f64) {
        if let Some(ts) = timestamp {
            let second_index = ts.as_secs();
            let fractional = ts
                .checked_sub(Duration::from_secs(second_index))
                .unwrap_or_else(|| Duration::from_secs(0))
                .as_secs_f64();
            return (second_index, fractional);
        }

        let samples = self.samples_per_second as u64;
        let processed = processed_index.saturating_sub(1);
        let second_index = processed / samples;
        let offset = processed.saturating_sub(second_index * samples);
        let elapsed = offset as f64 / self.samples_per_second as f64;
        (second_index, elapsed)
    }
}

struct FpsEstimator {
    last: Option<FpsObservation>,
    estimate: Option<f64>,
}

impl FpsEstimator {
    fn new() -> Self {
        Self {
            last: None,
            estimate: None,
        }
    }

    fn observe(&mut self, frame_index: u64, timestamp: Option<Duration>) -> Option<usize> {
        let Some(ts) = timestamp else {
            return self
                .estimate
                .map(|fps| capacity_from_fps(fps, DEFAULT_POOL_CAPACITY));
        };

        if let Some(previous) = self.last {
            if frame_index > previous.frame_index {
                if let Some(delta) = ts.checked_sub(previous.timestamp) {
                    let seconds = delta.as_secs_f64();
                    if seconds > 0.0 {
                        let frames = (frame_index - previous.frame_index) as f64;
                        let fps = frames / seconds;
                        self.estimate = Some(match self.estimate {
                            Some(current) => 0.8 * current + 0.2 * fps,
                            None => fps,
                        });
                    }
                }
            }
        }

        self.last = Some(FpsObservation {
            frame_index,
            timestamp: ts,
        });

        self.estimate
            .map(|fps| capacity_from_fps(fps, DEFAULT_POOL_CAPACITY))
    }
}

#[derive(Clone, Copy)]
struct FpsObservation {
    frame_index: u64,
    timestamp: Duration,
}

#[allow(dead_code)]
#[derive(Debug)]
struct PoolEntry {
    frame_index: u64,
    frame_type: FrameType,
    frame: Arc<YPlaneFrame>,
}

impl PoolEntry {
    fn new(frame_index: u64, frame_type: FrameType, frame: Arc<YPlaneFrame>) -> Self {
        Self {
            frame_index,
            frame_type,
            frame,
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct FrameHistory {
    entries: Arc<Vec<HistoryRecord>>,
}

impl FrameHistory {
    fn new(entries: Vec<HistoryRecord>) -> Self {
        Self {
            entries: Arc::new(entries),
        }
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[allow(dead_code)]
    pub fn records(&self) -> &[HistoryRecord] {
        self.entries.as_ref().as_slice()
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct HistoryRecord {
    pub frame_index: u64,
    pub frame_type: FrameType,
    frame: Arc<YPlaneFrame>,
}

impl HistoryRecord {
    #[allow(dead_code)]
    pub fn frame(&self) -> &YPlaneFrame {
        &self.frame
    }

    #[allow(dead_code)]
    pub fn frame_handle(&self) -> Arc<YPlaneFrame> {
        Arc::clone(&self.frame)
    }
}

fn capacity_from_fps(fps: f64, fallback: usize) -> usize {
    if fps.is_finite() && fps > 0.0 {
        let bounded = fps.clamp(1.0, MAX_POOL_CAPACITY as f64).ceil();
        bounded as usize
    } else {
        fallback
    }
}
