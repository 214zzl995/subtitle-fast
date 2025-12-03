use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{StreamExt, stream::unfold};
use tokio::sync::mpsc;

use super::StreamBundle;
use subtitle_fast_types::{YPlaneError, YPlaneFrame, YPlaneResult};

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

#[derive(Debug, Clone)]
pub struct SamplerContext {
    estimated_fps: Option<f64>,
}

impl SamplerContext {
    fn initial() -> Self {
        Self {
            estimated_fps: None,
        }
    }

    fn with_estimate(estimated_fps: f64) -> Self {
        Self {
            estimated_fps: Some(estimated_fps),
        }
    }

    #[allow(dead_code)]
    pub fn estimated_fps(&self) -> Option<f64> {
        self.estimated_fps
    }
}

pub struct SampledFrame {
    frame_index: u64,
    frame: Arc<YPlaneFrame>,
    history: FrameHistory,
    context: Arc<SamplerContext>,
}

impl SampledFrame {
    fn new(
        frame_index: u64,
        frame: Arc<YPlaneFrame>,
        history: FrameHistory,
        context: Arc<SamplerContext>,
    ) -> Self {
        Self {
            frame_index,
            frame,
            history,
            context,
        }
    }

    #[allow(dead_code)]
    pub fn frame(&self) -> &YPlaneFrame {
        self.frame.as_ref()
    }

    pub fn frame_handle(&self) -> Arc<YPlaneFrame> {
        Arc::clone(&self.frame)
    }

    #[allow(dead_code)]
    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }

    #[allow(dead_code)]
    pub fn into_frame(self) -> Arc<YPlaneFrame> {
        self.frame
    }

    #[allow(dead_code)]
    pub fn history(&self) -> &FrameHistory {
        &self.history
    }

    #[allow(dead_code)]
    pub fn sampler_context(&self) -> &SamplerContext {
        &self.context
    }
}

pub struct FrameSampler {
    samples_per_second: u32,
}

impl FrameSampler {
    pub fn new(samples_per_second: u32) -> Self {
        Self { samples_per_second }
    }
}

impl FrameSampler {
    pub fn attach(
        self,
        input: StreamBundle<YPlaneResult<YPlaneFrame>>,
    ) -> StreamBundle<SamplerResult> {
        let StreamBundle {
            stream,
            total_frames,
        } = input;

        let samples_per_second = self.samples_per_second;
        let (tx, rx) = mpsc::channel::<SamplerResult>(SAMPLER_CHANNEL_CAPACITY);

        tokio::spawn(async move {
            let mut upstream = stream;
            let mut worker = SamplerWorker::new(samples_per_second);

            while let Some(maybe_item) = upstream.next().await {
                match maybe_item {
                    Ok(frame) => {
                        if worker.handle_frame(frame, &tx).await.is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(Err(err)).await;
                        break;
                    }
                }
            }

            worker.emit_final_sample(&tx).await;
        });

        let stream = Box::pin(unfold(rx, |mut receiver| async {
            receiver.recv().await.map(|item| (item, receiver))
        }));

        StreamBundle::new(stream, total_frames)
    }
}

struct SamplerWorker {
    processed: u64,
    pool: SamplerPool,
    schedule: SampleSchedule,
    fps: FpsEstimator,
    context: Arc<SamplerContext>,
    last_sampled_index: Option<u64>,
}

impl SamplerWorker {
    fn new(samples_per_second: u32) -> Self {
        Self {
            processed: 0,
            pool: SamplerPool::new(DEFAULT_POOL_CAPACITY),
            schedule: SampleSchedule::new(samples_per_second),
            fps: FpsEstimator::new(),
            context: Arc::new(SamplerContext::initial()),
            last_sampled_index: None,
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

        let frame_type = if self.schedule.should_sample(timestamp, processed_index) {
            FrameType::Sampled
        } else {
            FrameType::Skipped
        };

        if let Some(fps) = self.fps.observe(frame_index, timestamp) {
            self.update_tuning(fps);
        }

        let frame_arc = Arc::new(frame);
        self.pool.push(PoolEntry::new(
            frame_index,
            frame_type,
            Arc::clone(&frame_arc),
        ));

        let history = if matches!(frame_type, FrameType::Sampled) {
            Some(self.pool.snapshot())
        } else {
            None
        };

        if let Some(history) = history {
            let sample = SampledFrame::new(frame_index, frame_arc, history, self.context.clone());
            if tx.send(Ok(sample)).await.is_err() {
                return Err(());
            }
            self.last_sampled_index = Some(frame_index);
        }

        Ok(())
    }

    async fn emit_final_sample(&mut self, tx: &mpsc::Sender<SamplerResult>) {
        let Some(latest) = self.pool.latest() else {
            return;
        };
        if self.last_sampled_index == Some(latest.frame_index) {
            return;
        }
        let history = self.pool.snapshot();
        let sample = SampledFrame::new(
            latest.frame_index,
            latest.frame_handle(),
            history,
            self.context.clone(),
        );
        let _ = tx.send(Ok(sample)).await;
        self.last_sampled_index = Some(latest.frame_index);
    }

    fn update_tuning(&mut self, fps: f64) {
        if let Some(current) = self.context.estimated_fps()
            && (current - fps).abs() <= EPSILON
        {
            return;
        }

        let mut capacity = if fps.is_finite() && fps > 0.0 {
            fps.ceil().max(1.0) as usize
        } else {
            DEFAULT_POOL_CAPACITY
        };
        if capacity > MAX_POOL_CAPACITY {
            capacity = MAX_POOL_CAPACITY;
        }
        self.pool.set_capacity(capacity);
        self.context = Arc::new(SamplerContext::with_estimate(fps));
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

    fn latest(&self) -> Option<&PoolEntry> {
        self.entries.back()
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

    fn observe(&mut self, frame_index: u64, timestamp: Option<Duration>) -> Option<f64> {
        let ts = match timestamp {
            Some(ts) => ts,
            None => return self.estimate,
        };

        if let Some(previous) = self.last
            && frame_index > previous.frame_index
            && let Some(delta) = ts.checked_sub(previous.timestamp)
        {
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

        self.last = Some(FpsObservation {
            frame_index,
            timestamp: ts,
        });

        self.estimate
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

    fn frame_handle(&self) -> Arc<YPlaneFrame> {
        Arc::clone(&self.frame)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn sampled_history_includes_current_frame() {
        let mut worker = SamplerWorker::new(1);
        let (tx, mut rx) = mpsc::channel(1);
        let frame = YPlaneFrame::from_owned(2, 2, 2, Some(Duration::from_millis(0)), vec![0; 4])
            .expect("frame");

        worker
            .handle_frame(frame, &tx)
            .await
            .expect("frame handled");

        let sample = rx
            .recv()
            .await
            .expect("sample missing")
            .expect("sample result");
        let frame_index = sample.frame_index();
        let has_current = sample
            .history()
            .records()
            .iter()
            .any(|record| record.frame_index == frame_index);

        assert!(has_current, "sample history should include current frame");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn final_sample_emitted_for_unsampled_tail() {
        let mut worker = SamplerWorker::new(1);
        let (tx, mut rx) = mpsc::channel(4);

        let frame_a =
            YPlaneFrame::from_owned(2, 2, 2, Some(Duration::from_millis(0)), vec![1; 4]).unwrap();
        let frame_b =
            YPlaneFrame::from_owned(2, 2, 2, Some(Duration::from_millis(10)), vec![2; 4]).unwrap();

        worker
            .handle_frame(frame_a, &tx)
            .await
            .expect("first frame");
        let first = rx.recv().await.expect("first sample").expect("ok");
        assert_eq!(first.frame_index(), 0);

        worker
            .handle_frame(frame_b, &tx)
            .await
            .expect("second frame");
        assert!(
            rx.try_recv().is_err(),
            "second frame should not be sampled yet"
        );

        worker.emit_final_sample(&tx).await;
        let final_sample = rx.recv().await.expect("final sample").expect("ok");
        assert_eq!(final_sample.frame_index(), 1);
        assert!(
            final_sample
                .history()
                .records()
                .iter()
                .any(|record| record.frame_index == 1),
            "final history should include latest frame"
        );
    }
}
