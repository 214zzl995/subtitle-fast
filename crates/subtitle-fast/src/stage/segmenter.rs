use std::sync::Arc;
use std::time::Duration;

use futures_util::{StreamExt, stream::unfold};
use tokio::sync::mpsc;

use super::StreamBundle;
use super::detector::{DetectionSample, DetectionSampleResult, DetectorError};
use super::sampler::{FrameHistory, SampledFrame};
use crate::settings::DetectionSettings;
use subtitle_fast_comparator::{
    ComparatorFactory, ComparatorKind, ComparatorSettings, FeatureBlob, SubtitleComparator,
};
use subtitle_fast_types::{DetectionRegion, RoiConfig, YPlaneFrame};

const SEGMENTER_CHANNEL_CAPACITY: usize = 4;

pub struct SubtitleInterval {
    pub start_time: Duration,
    pub end_time: Duration,
    pub start_frame: u64,
    pub roi: RoiConfig,
    pub first_yplane: Arc<YPlaneFrame>,
}

pub struct SegmenterEvent {
    pub sample: Option<DetectionSample>,
    pub intervals: Vec<SubtitleInterval>,
    pub segment_timings: Option<SegmentTimings>,
}

pub type SegmenterResult = Result<SegmenterEvent, SegmenterError>;

#[derive(Debug)]
pub enum SegmenterError {
    Detector(DetectorError),
}

pub struct SubtitleSegmenter {
    comparator_factory: ComparatorFactory,
    samples_per_second: u32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SegmentTimings {
    pub frames: u64,
    pub roi_extracts: u64,
    pub comparisons: u64,
    pub extract: Duration,
    pub compare: Duration,
    pub total: Duration,
}

impl SubtitleSegmenter {
    pub fn new(settings: &DetectionSettings) -> Self {
        let comparator_kind = settings.comparator.unwrap_or(ComparatorKind::BitsetCover);
        let comparator_settings = ComparatorSettings {
            kind: comparator_kind,
            target: settings.target,
            delta: settings.delta,
        };
        let comparator_factory = ComparatorFactory::new(comparator_settings);
        let samples_per_second = settings.samples_per_second;
        Self {
            comparator_factory,
            samples_per_second,
        }
    }

    pub fn attach(
        self,
        input: StreamBundle<DetectionSampleResult>,
    ) -> StreamBundle<SegmenterResult> {
        let StreamBundle {
            stream,
            total_frames,
        } = input;

        let comparator_factory = self.comparator_factory;
        let (tx, rx) = mpsc::channel::<SegmenterResult>(SEGMENTER_CHANNEL_CAPACITY);

        tokio::spawn(async move {
            let mut upstream = stream;
            let comparator = comparator_factory.build();
            let window_frames = window_frames(self.samples_per_second);
            let mut worker = SegmenterWorker::new(comparator, window_frames, window_frames);

            while let Some(event) = upstream.next().await {
                match event {
                    Ok(sample) => {
                        let started = std::time::Instant::now();
                        let mut timings = SegmentTimings::default();
                        let mut segment_event = worker.handle_sample(sample, &mut timings);
                        timings.total = started.elapsed();
                        timings.frames = 1;
                        segment_event.segment_timings = Some(timings);
                        if tx.send(Ok(segment_event)).await.is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let mut timings = SegmentTimings::default();
                        let flush_intervals = worker.flush_active_segments(&mut timings);
                        if !flush_intervals.is_empty() {
                            let _ = tx
                                .send(Ok(SegmenterEvent {
                                    sample: None,
                                    intervals: flush_intervals,
                                    segment_timings: None,
                                }))
                                .await;
                        }
                        let _ = tx.send(Err(SegmenterError::Detector(err))).await;
                        break;
                    }
                }
            }

            let mut timings = SegmentTimings::default();
            let flush_intervals = worker.flush_active_segments(&mut timings);
            if !flush_intervals.is_empty() {
                let _ = tx
                    .send(Ok(SegmenterEvent {
                        sample: None,
                        intervals: flush_intervals,
                        segment_timings: None,
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

struct SubtitleRegion {
    roi: RoiConfig,
}

struct SubtitleFrame {
    time: Duration,
    frame_index: u64,
    yplane: Arc<YPlaneFrame>,
    history: FrameHistory,
    regions: Vec<SubtitleRegion>,
}

struct ActiveSubtitle {
    roi: RoiConfig,
    template_yplane: Arc<YPlaneFrame>,
    template_features: FeatureBlob,
    anchor_features: Option<FeatureBlob>,
    start_time: Duration,
    start_frame: u64,
    last_time: Duration,
    last_frame: u64,
    consecutive_missing: u32,
}

struct PendingSubtitle {
    roi: RoiConfig,
    template_yplane: Arc<YPlaneFrame>,
    template_features: FeatureBlob,
    anchor_features: Option<FeatureBlob>,
    first_time: Duration,
    first_frame: u64,
    history: FrameHistory,
    hit_count: u32,
}

struct SegmenterWorker {
    comparator: Arc<dyn SubtitleComparator>,
    k_on: u32,
    k_off: u32,
    active: Vec<ActiveSubtitle>,
    pending: Vec<PendingSubtitle>,
    last_history: Option<FrameHistory>,
}

impl SegmenterWorker {
    fn new(comparator: Arc<dyn SubtitleComparator>, k_on: u32, k_off: u32) -> Self {
        Self {
            comparator,
            k_on: k_on.max(1),
            k_off: k_off.max(1),
            active: Vec::new(),
            pending: Vec::new(),
            last_history: None,
        }
    }

    fn handle_sample(
        &mut self,
        sample: DetectionSample,
        timings: &mut SegmentTimings,
    ) -> SegmenterEvent {
        let frame = self.build_frame(&sample);
        self.last_history = Some(frame.history.clone());

        let mut intervals = Vec::new();
        let roi_count = frame.regions.len();
        let comparator = self.comparator.clone();
        let comparator_ref: &dyn SubtitleComparator = &*comparator;
        let mut roi_features: Vec<Option<FeatureBlob>> = Vec::with_capacity(roi_count);
        for region in &frame.regions {
            let features = timed_extract(timings, comparator_ref, &frame.yplane, &region.roi);
            roi_features.push(features);
        }
        let mut roi_used: Vec<bool> = vec![false; roi_count];

        // Step 1: try to match existing active subtitles.
        let mut idx = 0;
        while idx < self.active.len() {
            let matched = match_active(
                comparator_ref,
                &mut self.active[idx],
                &frame,
                &roi_features,
                &mut roi_used,
                timings,
            );
            if matched {
                self.active[idx].consecutive_missing = 0;
                idx += 1;
            } else {
                let miss = self.active[idx].consecutive_missing.saturating_add(1);
                self.active[idx].consecutive_missing = miss;
                if miss > self.k_off {
                    let active = self.active.remove(idx);
                    intervals.push(self.close_active(active, timings));
                } else {
                    idx += 1;
                }
            }
        }

        // Step 2: update pending subtitles.
        let mut pidx = 0;
        while pidx < self.pending.len() {
            let hit = match_pending(
                comparator_ref,
                &mut self.pending[pidx],
                &frame,
                &roi_features,
                &mut roi_used,
                timings,
            );
            if hit {
                let pending = &mut self.pending[pidx];
                pending.hit_count = pending.hit_count.saturating_add(1);
                if pending.hit_count >= self.k_on {
                    let pending = self.pending.remove(pidx);
                    let active = self.promote_pending(pending, timings);
                    self.active.push(active);
                } else {
                    pidx += 1;
                }
            } else {
                // Drop pending candidates that no longer match.
                self.pending.remove(pidx);
            }
        }

        // Step 3: create new pending subtitles from unused regions.
        for (i, region) in frame.regions.iter().enumerate() {
            if roi_used.get(i).copied().unwrap_or(false) {
                continue;
            }
            let roi = region.roi;
            if let Some(features) = roi_features.get(i).and_then(|f| f.clone()) {
                let pending = PendingSubtitle {
                    roi,
                    template_yplane: Arc::clone(&frame.yplane),
                    template_features: features,
                    anchor_features: None,
                    first_time: frame.time,
                    first_frame: frame.frame_index,
                    history: frame.history.clone(),
                    hit_count: 1,
                };
                self.pending.push(pending);
            }
        }

        SegmenterEvent {
            sample: Some(sample),
            intervals,
            segment_timings: None,
        }
    }

    fn build_frame(&self, sample: &DetectionSample) -> SubtitleFrame {
        let frame_index = sample.sample.frame_index();
        let time = sample_time(&sample.sample);
        let yplane = sample.sample.frame_handle();
        let history = sample.sample.history().clone();
        let regions = sample
            .detection
            .regions
            .iter()
            .map(|region| SubtitleRegion {
                roi: region_to_roi(region, &yplane),
            })
            .collect();

        SubtitleFrame {
            time,
            frame_index,
            yplane,
            history,
            regions,
        }
    }

    fn promote_pending(
        &self,
        pending: PendingSubtitle,
        timings: &mut SegmentTimings,
    ) -> ActiveSubtitle {
        let (start_frame, start_time, template_yplane, template_features, anchor_features) =
            self.refine_start(&pending, timings);
        ActiveSubtitle {
            roi: pending.roi,
            template_yplane,
            template_features,
            anchor_features,
            start_time,
            start_frame,
            last_time: pending.first_time,
            last_frame: pending.first_frame,
            consecutive_missing: 0,
        }
    }

    fn refine_start(
        &self,
        pending: &PendingSubtitle,
        timings: &mut SegmentTimings,
    ) -> (
        u64,
        Duration,
        Arc<YPlaneFrame>,
        FeatureBlob,
        Option<FeatureBlob>,
    ) {
        let mut best_frame = pending.first_frame;
        let mut best_time = pending.first_time;
        let mut best_yplane = Arc::clone(&pending.template_yplane);
        let mut best_features = pending.template_features.clone();
        let mut search_anchor = pending.anchor_features.clone();
        let mut anchor_for_state = pending.anchor_features.clone();

        for record in pending.history.records().iter().rev() {
            if record.frame_index >= pending.first_frame {
                continue;
            }
            let frame_index = record.frame_index;
            let Some(candidate_features) = timed_extract(
                timings,
                self.comparator.as_ref(),
                record.frame(),
                &pending.roi,
            ) else {
                continue;
            };
            let reference = comparison_anchor(&search_anchor, &pending.template_features);
            let report = timed_compare(
                timings,
                self.comparator.as_ref(),
                reference,
                &candidate_features,
            );
            if report.same_segment {
                search_anchor = Some(candidate_features.clone());
                if anchor_for_state.is_none() {
                    anchor_for_state = Some(candidate_features.clone());
                }
                best_frame = frame_index;
                if let Some(ts) = record.frame().timestamp() {
                    best_time = ts;
                }
                best_yplane = record.frame_handle();
                best_features = candidate_features;
            }
        }

        (
            best_frame,
            best_time,
            best_yplane,
            best_features,
            anchor_for_state,
        )
    }

    fn close_active(
        &self,
        active: ActiveSubtitle,
        timings: &mut SegmentTimings,
    ) -> SubtitleInterval {
        let end_time = if let Some(history) = &self.last_history {
            self.refine_end(&active, history, timings)
        } else {
            active.last_time
        };

        SubtitleInterval {
            start_time: active.start_time,
            end_time,
            start_frame: active.start_frame,
            roi: active.roi,
            first_yplane: active.template_yplane,
        }
    }

    fn refine_end(
        &self,
        active: &ActiveSubtitle,
        history: &FrameHistory,
        timings: &mut SegmentTimings,
    ) -> Duration {
        let mut best_frame = active.last_frame;
        let mut best_time = active.last_time;
        let mut search_anchor = active.anchor_features.clone();
        for record in history.records() {
            if record.frame_index <= active.last_frame {
                continue;
            }
            let Some(candidate_features) = timed_extract(
                timings,
                self.comparator.as_ref(),
                record.frame(),
                &active.roi,
            ) else {
                continue;
            };
            let reference = comparison_anchor(&search_anchor, &active.template_features);
            let report = timed_compare(
                timings,
                self.comparator.as_ref(),
                reference,
                &candidate_features,
            );
            if report.same_segment {
                search_anchor = Some(candidate_features.clone());
                best_frame = record.frame_index;
                if let Some(ts) = record.frame().timestamp() {
                    best_time = ts;
                }
            }
        }

        let mut next_timestamp = None;
        let mut prev_timestamp = None;
        for record in history.records() {
            if record.frame_index < best_frame {
                if let Some(ts) = record.frame().timestamp() {
                    prev_timestamp = Some(ts);
                }
                continue;
            }
            if record.frame_index > best_frame {
                if let Some(ts) = record.frame().timestamp() {
                    next_timestamp = Some(ts);
                }
                break;
            }
        }

        if let Some(next_ts) = next_timestamp {
            next_ts
        } else if let Some(prev) = prev_timestamp {
            if let Some(delta) = best_time.checked_sub(prev) {
                best_time.checked_add(delta).unwrap_or(best_time)
            } else {
                best_time
            }
        } else {
            best_time
        }
    }

    fn flush_active_segments(&mut self, timings: &mut SegmentTimings) -> Vec<SubtitleInterval> {
        let mut intervals = Vec::new();
        while let Some(active) = self.active.pop() {
            intervals.push(self.close_active(active, timings));
        }
        self.pending.clear();
        intervals
    }
}

fn comparison_anchor<'a>(
    anchor: &'a Option<FeatureBlob>,
    template: &'a FeatureBlob,
) -> &'a FeatureBlob {
    anchor.as_ref().unwrap_or(template)
}

fn timed_extract(
    timings: &mut SegmentTimings,
    comparator: &dyn SubtitleComparator,
    frame: &YPlaneFrame,
    roi: &RoiConfig,
) -> Option<FeatureBlob> {
    let started = std::time::Instant::now();
    let result = comparator.extract(frame, roi);
    timings.roi_extracts = timings.roi_extracts.saturating_add(1);
    timings.extract = timings.extract.saturating_add(started.elapsed());
    result
}

fn timed_compare(
    timings: &mut SegmentTimings,
    comparator: &dyn SubtitleComparator,
    reference: &FeatureBlob,
    candidate: &FeatureBlob,
) -> subtitle_fast_comparator::pipeline::ComparisonReport {
    let started = std::time::Instant::now();
    let report = comparator.compare(reference, candidate);
    timings.comparisons = timings.comparisons.saturating_add(1);
    timings.compare = timings.compare.saturating_add(started.elapsed());
    report
}

fn region_to_roi(region: &DetectionRegion, frame: &YPlaneFrame) -> RoiConfig {
    let fw = frame.width().max(1) as f32;
    let fh = frame.height().max(1) as f32;
    let x0 = (region.x / fw).clamp(0.0, 1.0);
    let x1 = ((region.x + region.width) / fw).clamp(x0, 1.0);
    let y0 = (region.y / fh).clamp(0.0, 1.0);
    let y1 = ((region.y + region.height) / fh).clamp(y0, 1.0);
    RoiConfig {
        x: x0,
        y: y0,
        width: (x1 - x0).max(0.0),
        height: (y1 - y0).max(0.0),
    }
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

fn window_frames(samples_per_second: u32) -> u32 {
    std::cmp::max(1, samples_per_second.div_ceil(5))
}

fn overlaps_vertically(a: &RoiConfig, b: &RoiConfig) -> bool {
    let a_top = a.y;
    let a_bottom = a.y + a.height;
    let b_top = b.y;
    let b_bottom = b.y + b.height;
    a_top <= b_bottom && b_top <= a_bottom
}

fn match_active(
    comparator: &dyn SubtitleComparator,
    active: &mut ActiveSubtitle,
    frame: &SubtitleFrame,
    roi_features: &[Option<FeatureBlob>],
    roi_used: &mut [bool],
    timings: &mut SegmentTimings,
) -> bool {
    for (idx, region) in frame.regions.iter().enumerate() {
        if roi_used.get(idx).copied().unwrap_or(false) {
            continue;
        }
        if !overlaps_vertically(&active.roi, &region.roi) {
            continue;
        }
        let Some(candidate) = roi_features.get(idx).and_then(|f| f.clone()) else {
            continue;
        };
        let reference = comparison_anchor(&active.anchor_features, &active.template_features);
        let report = timed_compare(timings, comparator, reference, &candidate);
        if report.same_segment {
            active.roi = region.roi;
            active.last_time = frame.time;
            active.last_frame = frame.frame_index;
            active.anchor_features = Some(candidate);
            if let Some(slot) = roi_used.get_mut(idx) {
                *slot = true;
            }
            return true;
        }
    }
    false
}

fn match_pending(
    comparator: &dyn SubtitleComparator,
    pending: &mut PendingSubtitle,
    frame: &SubtitleFrame,
    roi_features: &[Option<FeatureBlob>],
    roi_used: &mut [bool],
    timings: &mut SegmentTimings,
) -> bool {
    for (idx, region) in frame.regions.iter().enumerate() {
        if roi_used.get(idx).copied().unwrap_or(false) {
            continue;
        }
        if !overlaps_vertically(&pending.roi, &region.roi) {
            continue;
        }
        let Some(candidate) = roi_features.get(idx).and_then(|f| f.clone()) else {
            continue;
        };
        let reference = comparison_anchor(&pending.anchor_features, &pending.template_features);
        let report = timed_compare(timings, comparator, reference, &candidate);
        if report.same_segment {
            pending.anchor_features = Some(candidate);
            if let Some(slot) = roi_used.get_mut(idx) {
                *slot = true;
            }
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;
    use subtitle_fast_comparator::pipeline::ComparisonReport;

    struct TagComparator;

    impl SubtitleComparator for TagComparator {
        fn name(&self) -> &'static str {
            "tag-comparator"
        }

        fn extract(&self, _frame: &YPlaneFrame, _roi: &RoiConfig) -> Option<FeatureBlob> {
            Some(FeatureBlob::new("tag", ()))
        }

        fn compare(&self, reference: &FeatureBlob, candidate: &FeatureBlob) -> ComparisonReport {
            let same = reference.tag() == candidate.tag();
            ComparisonReport::new(if same { 1.0 } else { 0.0 }, same)
        }
    }

    #[test]
    fn window_frames_approximates_200ms() {
        assert_eq!(window_frames(0), 1);
        assert_eq!(window_frames(1), 1);
        assert_eq!(window_frames(5), 1);
        assert_eq!(window_frames(10), 2);
        assert_eq!(window_frames(25), 5);
    }

    #[test]
    fn match_active_prefers_anchor_and_updates_it() {
        let comparator = TagComparator;
        let template_features = FeatureBlob::new("one", ());
        let anchor_features = Some(FeatureBlob::new("two", ()));
        let roi = RoiConfig {
            x: 0.0,
            y: 0.1,
            width: 0.5,
            height: 0.2,
        };
        let template_yplane = Arc::new(
            YPlaneFrame::from_owned(1, 1, 1, Some(Duration::from_secs(0)), vec![0])
                .expect("yplane"),
        );
        let mut active = ActiveSubtitle {
            roi,
            template_yplane: Arc::clone(&template_yplane),
            template_features,
            anchor_features,
            start_time: Duration::from_secs(0),
            start_frame: 0,
            last_time: Duration::from_secs(0),
            last_frame: 0,
            consecutive_missing: 0,
        };
        let frame = SubtitleFrame {
            time: Duration::from_secs(1),
            frame_index: 1,
            yplane: Arc::clone(&template_yplane),
            history: FrameHistory::new(Vec::new()),
            regions: vec![SubtitleRegion { roi }],
        };
        let roi_features = vec![Some(FeatureBlob::new("two", ()))];
        let mut roi_used = vec![false];
        let mut timings = SegmentTimings::default();

        let matched = match_active(
            &comparator,
            &mut active,
            &frame,
            &roi_features,
            &mut roi_used,
            &mut timings,
        );

        assert!(matched, "anchor reference should accept matching tags");
        assert!(roi_used[0], "matched ROI should be marked used");
        assert_eq!(active.last_frame, 1);
        assert_eq!(
            active.anchor_features.as_ref().expect("anchor set").tag(),
            "two"
        );
    }

    #[test]
    fn match_pending_falls_back_to_template_until_anchor_exists() {
        let comparator = TagComparator;
        let template_features = FeatureBlob::new("pending", ());
        let roi = RoiConfig {
            x: 0.0,
            y: 0.2,
            width: 0.5,
            height: 0.2,
        };
        let template_yplane = Arc::new(
            YPlaneFrame::from_owned(1, 1, 1, Some(Duration::from_secs(0)), vec![0])
                .expect("yplane"),
        );
        let mut pending = PendingSubtitle {
            roi,
            template_yplane: Arc::clone(&template_yplane),
            template_features,
            anchor_features: None,
            first_time: Duration::from_secs(0),
            first_frame: 0,
            history: FrameHistory::new(Vec::new()),
            hit_count: 0,
        };
        let frame = SubtitleFrame {
            time: Duration::from_secs(1),
            frame_index: 1,
            yplane: Arc::clone(&template_yplane),
            history: FrameHistory::new(Vec::new()),
            regions: vec![SubtitleRegion { roi }],
        };
        let roi_features = vec![Some(FeatureBlob::new("pending", ()))];
        let mut roi_used = vec![false];
        let mut timings = SegmentTimings::default();

        let matched = match_pending(
            &comparator,
            &mut pending,
            &frame,
            &roi_features,
            &mut roi_used,
            &mut timings,
        );

        assert!(matched, "template fallback should allow first anchor");
        assert!(roi_used[0], "matched ROI should be marked used");
        assert_eq!(
            pending
                .anchor_features
                .as_ref()
                .expect("anchor set from match")
                .tag(),
            "pending"
        );
    }
}
