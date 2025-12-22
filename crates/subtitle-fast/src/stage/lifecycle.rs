use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream::unfold};
use tokio::sync::mpsc;

use super::StreamBundle;
use super::detector::DetectionSample;
use super::determiner::{
    RegionDeterminerError, RegionDeterminerEvent, RegionDeterminerResult, RegionId,
};
use super::sampler::{FrameHistory, SampledFrame, SamplerContext};
use crate::settings::DetectionSettings;
use subtitle_fast_comparator::{
    ComparatorFactory, ComparatorKind, ComparatorSettings, FeatureBlob, SubtitleComparator,
};
use subtitle_fast_types::{RoiConfig, VideoFrame};

const REGION_TRACKER_CHANNEL_CAPACITY: usize = 4;
const MIN_REGION_AREA_FRACTION: f32 = 0.001;
const MIN_REGION_DURATION: Duration = Duration::from_millis(200);
const MIN_REGION_DIM_PX: u32 = 15;

pub struct CompletedRegion {
    pub id: RegionId,
    pub label: String,
    pub start_time: Duration,
    pub end_time: Duration,
    pub start_frame: u64,
    pub end_frame: u64,
    pub roi: RoiConfig,
    pub frame: Arc<VideoFrame>,
}

pub struct LifecycleEvent {
    pub sample: Option<DetectionSample>,
    pub completed: Vec<CompletedRegion>,
    pub region_timings: Option<RegionTimings>,
}

pub type LifecycleResult = Result<LifecycleEvent, RegionLifecycleError>;

#[derive(Debug)]
pub enum RegionLifecycleError {
    Determiner(RegionDeterminerError),
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RegionTimings {
    pub frames: u64,
    pub roi_extracts: u64,
    pub comparisons: u64,
    pub extract: Duration,
    pub compare: Duration,
    pub total: Duration,
}

pub struct RegionLifecycleTracker {
    comparator_factory: ComparatorFactory,
}

impl RegionLifecycleTracker {
    pub fn new(settings: &DetectionSettings) -> Self {
        let comparator_kind = settings.comparator.unwrap_or(ComparatorKind::BitsetCover);
        let comparator_settings = ComparatorSettings {
            kind: comparator_kind,
            target: settings.target,
            delta: settings.delta,
        };
        let comparator_factory = ComparatorFactory::new(comparator_settings);
        Self { comparator_factory }
    }

    pub fn attach(
        self,
        input: StreamBundle<RegionDeterminerResult>,
    ) -> StreamBundle<LifecycleResult> {
        let StreamBundle {
            stream,
            total_frames,
        } = input;

        let comparator_factory = self.comparator_factory;
        let (tx, rx) = mpsc::channel::<LifecycleResult>(REGION_TRACKER_CHANNEL_CAPACITY);

        tokio::spawn(async move {
            let comparator = comparator_factory.build();
            let mut worker = RegionLifecycleWorker::new(comparator);
            let mut upstream = stream;

            while let Some(event) = upstream.next().await {
                match event {
                    Ok(regions) => {
                        let started = Instant::now();
                        let mut timings = RegionTimings::default();
                        let mut lifecycle_event = worker.handle_event(regions, &mut timings);
                        timings.total = started.elapsed();
                        timings.frames = 1;
                        lifecycle_event.region_timings = Some(timings);
                        if tx.send(Ok(lifecycle_event)).await.is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let mut timings = RegionTimings::default();
                        let flush = worker.flush_active(&mut timings);
                        if !flush.is_empty() {
                            let _ = tx
                                .send(Ok(LifecycleEvent {
                                    sample: None,
                                    completed: flush,
                                    region_timings: None,
                                }))
                                .await;
                        }
                        let _ = tx.send(Err(RegionLifecycleError::Determiner(err))).await;
                        break;
                    }
                }
            }

            let mut timings = RegionTimings::default();
            let flush = worker.flush_active(&mut timings);
            if !flush.is_empty() {
                let _ = tx
                    .send(Ok(LifecycleEvent {
                        sample: None,
                        completed: flush,
                        region_timings: None,
                    }))
                    .await;
            }
        });

        let stream = Box::pin(unfold(rx, |mut receiver| async {
            receiver.recv().await.map(|item| (item, receiver))
        }));

        StreamBundle::new(stream, total_frames)
    }
}

struct ActiveRegion {
    id: RegionId,
    label: String,
    roi: RoiConfig,
    template_features: FeatureBlob,
    anchor_features: Option<FeatureBlob>,
    start_time: Duration,
    start_frame: u64,
    last_time: Duration,
    last_frame: u64,
    frame: Arc<VideoFrame>,
}

struct RegionLifecycleWorker {
    comparator: Arc<dyn SubtitleComparator>,
    active: HashMap<RegionId, ActiveRegion>,
    last_history: Option<FrameHistory>,
}

impl RegionLifecycleWorker {
    fn new(comparator: Arc<dyn SubtitleComparator>) -> Self {
        Self {
            comparator,
            active: HashMap::new(),
            last_history: None,
        }
    }

    fn handle_event(
        &mut self,
        event: RegionDeterminerEvent,
        timings: &mut RegionTimings,
    ) -> LifecycleEvent {
        let frame_ctx = FrameContext::from_sample(&event.sample);
        self.last_history = Some(frame_ctx.history.clone());

        let mut roi_features: Vec<Option<FeatureBlob>> = Vec::with_capacity(event.regions.len());
        for region in &event.regions {
            let features = timed_extract(
                timings,
                self.comparator.as_ref(),
                &frame_ctx.frame,
                &region.roi,
            );
            roi_features.push(features);
        }

        let mut completed = Vec::new();
        let mut seen: HashSet<RegionId> = HashSet::new();

        for (idx, region) in event.regions.iter().enumerate() {
            let Some(features) = roi_features.get(idx).and_then(|f| f.clone()) else {
                continue;
            };
            if let Some(active) = self.active.get(&region.id) {
                let matched = match_active(
                    self.comparator.as_ref(),
                    active,
                    &frame_ctx,
                    &region.roi,
                    &features,
                    timings,
                );
                if matched {
                    if let Some(active) = self.active.get_mut(&region.id) {
                        active.roi = region.roi;
                        active.frame = Arc::clone(&frame_ctx.frame);
                        active.last_time = frame_ctx.time;
                        active.last_frame = frame_ctx.frame_index;
                        active.template_features = features.clone();
                        active.anchor_features = Some(features);
                    }
                    seen.insert(region.id);
                } else {
                    // Keep the existing active region; treat this as a non-updating observation.
                    seen.insert(region.id);
                }
            } else {
                let active = self.start_region(region, frame_ctx.clone(), features, timings);
                self.active.insert(region.id, active);
                seen.insert(region.id);
            }
        }

        let missing: Vec<RegionId> = self
            .active
            .keys()
            .copied()
            .filter(|id| !seen.contains(id))
            .collect();
        for id in missing {
            if let Some(done) = self.close_by_id(id, frame_ctx.history.clone(), timings) {
                completed.push(done);
            }
        }

        LifecycleEvent {
            sample: Some(event.sample),
            completed,
            region_timings: None,
        }
    }

    fn start_region(
        &self,
        region: &super::determiner::RegionUnit,
        frame: FrameContext,
        features: FeatureBlob,
        timings: &mut RegionTimings,
    ) -> ActiveRegion {
        let (start_frame, start_time, template_features, anchor_features) = determine_start(
            self.comparator.as_ref(),
            &frame,
            &region.roi,
            &features,
            timings,
        );

        ActiveRegion {
            id: region.id,
            label: region.label.clone(),
            roi: region.roi,
            template_features,
            anchor_features,
            start_time,
            start_frame,
            last_time: frame.time,
            last_frame: frame.frame_index,
            frame: frame.frame,
        }
    }

    fn close_by_id(
        &mut self,
        id: RegionId,
        history: FrameHistory,
        timings: &mut RegionTimings,
    ) -> Option<CompletedRegion> {
        let active = self.active.remove(&id)?;
        let completed = self.close_active(active, &history, timings);
        is_valid_region(&completed).then_some(completed)
    }

    fn close_active(
        &self,
        active: ActiveRegion,
        history: &FrameHistory,
        timings: &mut RegionTimings,
    ) -> CompletedRegion {
        let (end_time, end_frame, frame_handle) =
            refine_end(self.comparator.as_ref(), &active, history, timings);

        CompletedRegion {
            id: active.id,
            label: active.label,
            start_time: active.start_time,
            end_time,
            start_frame: active.start_frame,
            end_frame,
            roi: active.roi,
            frame: frame_handle,
        }
    }

    fn flush_active(&mut self, timings: &mut RegionTimings) -> Vec<CompletedRegion> {
        let mut completed = Vec::new();
        let history = self
            .last_history
            .clone()
            .unwrap_or_else(|| FrameHistory::new(Vec::new()));
        let ids: Vec<RegionId> = self.active.keys().copied().collect();
        for id in ids {
            if let Some(done) = self.close_by_id(id, history.clone(), timings) {
                completed.push(done);
            }
        }
        completed
    }
}

#[derive(Clone)]
struct FrameContext {
    time: Duration,
    frame_index: u64,
    frame: Arc<VideoFrame>,
    history: FrameHistory,
    sampler_context: SamplerContext,
}

impl FrameContext {
    fn from_sample(sample: &DetectionSample) -> Self {
        let time = sample_time(&sample.sample);
        let frame_index = sample.sample.frame_index();
        let frame = sample.sample.frame_handle();
        let history = sample.sample.history().clone();
        let sampler_context = sample.sample.sampler_context().clone();
        Self {
            time,
            frame_index,
            frame,
            history,
            sampler_context,
        }
    }
}

fn determine_start(
    comparator: &dyn SubtitleComparator,
    frame: &FrameContext,
    roi: &RoiConfig,
    features: &FeatureBlob,
    timings: &mut RegionTimings,
) -> (u64, Duration, FeatureBlob, Option<FeatureBlob>) {
    let mut best_frame = frame.frame_index;
    let mut best_time = frame.time;
    let mut template_features = features.clone();
    let mut anchor_features = None;

    for record in frame.history.records().iter().rev() {
        if record.frame_index >= frame.frame_index {
            continue;
        }
        let Some(candidate) = timed_extract(timings, comparator, record.frame(), roi) else {
            continue;
        };
        let reference = comparison_anchor(&anchor_features, features);
        let report = timed_compare(timings, comparator, reference, &candidate);
        if report.same_segment {
            best_frame = record.frame_index;
            best_time = frame_time(record.frame(), record.frame_index, &frame.sampler_context)
                .unwrap_or(best_time);
            if anchor_features.is_none() {
                anchor_features = Some(candidate.clone());
            }
            template_features = candidate;
        }
    }

    (best_frame, best_time, template_features, anchor_features)
}

fn refine_end(
    comparator: &dyn SubtitleComparator,
    active: &ActiveRegion,
    history: &FrameHistory,
    timings: &mut RegionTimings,
) -> (Duration, u64, Arc<VideoFrame>) {
    let mut best_frame = active.last_frame;
    let mut best_time = active.last_time;
    let mut best_frame_handle = Arc::clone(&active.frame);
    let mut anchor = active.anchor_features.clone();

    for record in history.records() {
        if record.frame_index <= active.last_frame {
            continue;
        }
        let Some(candidate) = timed_extract(timings, comparator, record.frame(), &active.roi)
        else {
            continue;
        };
        let reference = comparison_anchor(&anchor, &active.template_features);
        let report = timed_compare(timings, comparator, reference, &candidate);
        if report.same_segment {
            anchor = Some(candidate.clone());
            best_frame = record.frame_index;
            best_time = record.frame().timestamp().unwrap_or(best_time);
            best_frame_handle = record.frame_handle();
        }
    }

    (best_time, best_frame, best_frame_handle)
}

fn match_active(
    comparator: &dyn SubtitleComparator,
    active: &ActiveRegion,
    _frame: &FrameContext,
    _roi: &RoiConfig,
    candidate: &FeatureBlob,
    timings: &mut RegionTimings,
) -> bool {
    let reference = comparison_anchor(&active.anchor_features, &active.template_features);
    let report = timed_compare(timings, comparator, reference, candidate);
    report.same_segment
}

fn comparison_anchor<'a>(
    anchor: &'a Option<FeatureBlob>,
    template: &'a FeatureBlob,
) -> &'a FeatureBlob {
    anchor.as_ref().unwrap_or(template)
}

fn timed_extract(
    timings: &mut RegionTimings,
    comparator: &dyn SubtitleComparator,
    frame: &VideoFrame,
    roi: &RoiConfig,
) -> Option<FeatureBlob> {
    let started = Instant::now();
    let result = comparator.extract(frame, roi);
    timings.roi_extracts = timings.roi_extracts.saturating_add(1);
    timings.extract = timings.extract.saturating_add(started.elapsed());
    result
}

fn timed_compare(
    timings: &mut RegionTimings,
    comparator: &dyn SubtitleComparator,
    reference: &FeatureBlob,
    candidate: &FeatureBlob,
) -> subtitle_fast_comparator::pipeline::ComparisonReport {
    let started = Instant::now();
    let report = comparator.compare(reference, candidate);
    timings.comparisons = timings.comparisons.saturating_add(1);
    timings.compare = timings.compare.saturating_add(started.elapsed());
    report
}

fn sample_time(sample: &SampledFrame) -> Duration {
    if let Some(ts) = sample.frame().timestamp() {
        return ts;
    }
    if let Some(fps) = sample.sampler_context().estimated_fps()
        && fps > 0.0
    {
        let secs = sample.frame_index() as f64 / fps;
        return Duration::from_secs_f64(secs.max(0.0));
    }
    Duration::from_secs(0)
}

fn frame_time(frame: &VideoFrame, frame_index: u64, context: &SamplerContext) -> Option<Duration> {
    if let Some(ts) = frame.timestamp() {
        return Some(ts);
    }
    let fps = context.estimated_fps()?;
    if fps <= 0.0 {
        return None;
    }
    Some(Duration::from_secs_f64(frame_index as f64 / fps))
}

fn is_valid_region(region: &CompletedRegion) -> bool {
    let area = (region.roi.width.max(0.0)) * (region.roi.height.max(0.0));
    if area < MIN_REGION_AREA_FRACTION {
        return false;
    }
    let fw = region.frame.width().max(1);
    let fh = region.frame.height().max(1);
    let width_px = (region.roi.width.max(0.0) * fw as f32).floor() as u32;
    let height_px = (region.roi.height.max(0.0) * fh as f32).floor() as u32;
    if width_px < MIN_REGION_DIM_PX || height_px < MIN_REGION_DIM_PX {
        return false;
    }
    let duration = region
        .end_time
        .checked_sub(region.start_time)
        .unwrap_or_else(|| Duration::from_secs(0));
    duration >= MIN_REGION_DURATION
}
