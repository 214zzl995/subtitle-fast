use serde::Serialize;
use subtitle_fast_decoder::YPlaneFrame;
use subtitle_fast_validator::subtitle_detection::{
    DetectionRegion, RoiConfig, SubtitleDetectionResult,
};

use crate::stage::detection::SubtitleSegment;

use super::util::{duration_millis, duration_secs};
use std::cmp::Ordering;

#[derive(Clone)]
pub struct FrameAnalysisSample {
    pub frame: YPlaneFrame,
    pub detection: SubtitleDetectionResult,
    pub roi: Option<RoiConfig>,
}

#[derive(Debug, Serialize, Clone)]
pub(crate) struct FrameJsonRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) frame_index: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) timestamp: Option<f64>,
    pub(crate) has_subtitle: bool,
    pub(crate) max_score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) roi: Option<RoiJson>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) regions: Vec<DetectionRegion>,
}

impl FrameJsonRecord {
    pub(crate) fn from_sample(sample: &FrameAnalysisSample) -> Self {
        let FrameAnalysisSample {
            frame,
            detection,
            roi,
        } = sample;
        Self {
            frame_index: frame.frame_index(),
            timestamp: frame.timestamp().map(duration_secs),
            has_subtitle: detection.has_subtitle,
            max_score: detection.max_score,
            roi: roi.map(RoiJson::from),
            regions: detection.regions.clone(),
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub(crate) struct SegmentJsonRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) frame_index: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) frame_timestamp: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) start: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) end: Option<f64>,
    pub(crate) max_score: f32,
    pub(crate) region: RoiJson,
}

impl SegmentJsonRecord {
    pub(crate) fn from_segment(segment: &SubtitleSegment) -> Self {
        Self {
            frame_index: segment.frame.frame_index(),
            frame_timestamp: segment.frame.timestamp().map(duration_secs),
            start: segment.start.map(duration_secs),
            end: segment.end.map(duration_secs),
            max_score: segment.max_score,
            region: RoiJson::from(segment.region),
        }
    }
}

#[derive(Debug, Serialize, Clone, Copy)]
pub(crate) struct RoiJson {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

impl From<RoiConfig> for RoiJson {
    fn from(value: RoiConfig) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        }
    }
}

pub(crate) struct FrameSortKey {
    missing_index: bool,
    index: u64,
    timestamp: f64,
}

pub(crate) fn frame_sort_key(record: &FrameJsonRecord) -> FrameSortKey {
    FrameSortKey {
        missing_index: record.frame_index.is_none(),
        index: record.frame_index.unwrap_or(u64::MAX),
        timestamp: record.timestamp.unwrap_or(f64::MAX),
    }
}

impl PartialEq for FrameSortKey {
    fn eq(&self, other: &Self) -> bool {
        self.missing_index == other.missing_index
            && self.index == other.index
            && self.timestamp == other.timestamp
    }
}

impl Eq for FrameSortKey {}

impl PartialOrd for FrameSortKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FrameSortKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.missing_index
            .cmp(&other.missing_index)
            .then_with(|| self.index.cmp(&other.index))
            .then_with(|| {
                self.timestamp
                    .partial_cmp(&other.timestamp)
                    .unwrap_or(Ordering::Equal)
            })
    }
}

pub(crate) struct SegmentSortKey {
    start: f64,
    frame_timestamp: f64,
    frame_index: u64,
}

pub(crate) fn segment_sort_key(record: &SegmentJsonRecord) -> SegmentSortKey {
    SegmentSortKey {
        start: record.start.unwrap_or(f64::MAX),
        frame_timestamp: record.frame_timestamp.unwrap_or(f64::MAX),
        frame_index: record.frame_index.unwrap_or(u64::MAX),
    }
}

impl PartialEq for SegmentSortKey {
    fn eq(&self, other: &Self) -> bool {
        self.start == other.start
            && self.frame_timestamp == other.frame_timestamp
            && self.frame_index == other.frame_index
    }
}

impl Eq for SegmentSortKey {}

impl PartialOrd for SegmentSortKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SegmentSortKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.start
            .partial_cmp(&other.start)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                self.frame_timestamp
                    .partial_cmp(&other.frame_timestamp)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| self.frame_index.cmp(&other.frame_index))
    }
}

pub(crate) fn frame_identifier(frame: &YPlaneFrame) -> u64 {
    frame
        .frame_index()
        .or_else(|| frame.timestamp().map(duration_millis))
        .unwrap_or_default()
}
