use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream::unfold};
use tokio::sync::mpsc;

use super::StreamBundle;
use super::detector::DetectionSample;
use super::segmenter::{
    SegmentTimings, SegmenterError, SegmenterEvent, SegmenterResult, SubtitleInterval,
};
use subtitle_fast_decoder::YPlaneFrame;
use subtitle_fast_ocr::{LumaPlane, OcrEngine, OcrError, OcrRegion, OcrRequest, OcrResponse};
use subtitle_fast_validator::subtitle_detection::RoiConfig;

const OCR_CHANNEL_CAPACITY: usize = 4;

pub type OcrStageResult = Result<OcrEvent, OcrStageError>;

pub struct SubtitleOcr {
    engine: Arc<dyn OcrEngine>,
}

impl SubtitleOcr {
    pub fn new(engine: Arc<dyn OcrEngine>) -> Self {
        Self { engine }
    }

    pub fn attach(self, input: StreamBundle<SegmenterResult>) -> StreamBundle<OcrStageResult> {
        let StreamBundle {
            stream,
            total_frames,
        } = input;

        let engine = self.engine;
        let (tx, rx) = mpsc::channel::<OcrStageResult>(OCR_CHANNEL_CAPACITY);

        tokio::spawn(async move {
            if let Err(err) = engine.warm_up() {
                let _ = tx.send(Err(OcrStageError::Engine(err))).await;
                return;
            }

            let mut upstream = stream;
            let worker = OcrWorker::new(engine);

            while let Some(event) = upstream.next().await {
                match event {
                    Ok(segment_event) => match worker.handle_event(segment_event) {
                        Ok(ocr_event) => {
                            if tx.send(Ok(ocr_event)).await.is_err() {
                                break;
                            }
                        }
                        Err(err) => {
                            let _ = tx.send(Err(err)).await;
                            break;
                        }
                    },
                    Err(err) => {
                        let _ = tx.send(Err(OcrStageError::Segmenter(err))).await;
                        break;
                    }
                }
            }
        });

        let stream = Box::pin(unfold(rx, |mut receiver| async {
            match receiver.recv().await {
                Some(item) => Some((item, receiver)),
                None => None,
            }
        }));

        StreamBundle::new(stream, total_frames)
    }
}

pub struct OcrEvent {
    pub sample: Option<DetectionSample>,
    pub subtitles: Vec<OcredSubtitle>,
    pub segment_timings: Option<SegmentTimings>,
    pub timings: Option<OcrTimings>,
}

pub struct OcredSubtitle {
    pub interval: SubtitleInterval,
    #[allow(dead_code)]
    pub region: OcrRegion,
    pub response: OcrResponse,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct OcrTimings {
    pub intervals: u64,
    pub ocr_calls: u64,
    pub ocr_duration: Duration,
    pub total: Duration,
}

#[derive(Debug)]
pub enum OcrStageError {
    Segmenter(SegmenterError),
    Engine(OcrError),
}

struct OcrWorker {
    engine: Arc<dyn OcrEngine>,
}

impl OcrWorker {
    fn new(engine: Arc<dyn OcrEngine>) -> Self {
        Self { engine }
    }

    fn handle_event(&self, event: SegmenterEvent) -> Result<OcrEvent, OcrStageError> {
        let started = Instant::now();
        let mut timings = OcrTimings::default();
        let mut subtitles = Vec::with_capacity(event.intervals.len());

        for interval in event.intervals {
            let region = roi_to_region(&interval.roi, &interval.first_yplane);
            if is_low_contrast_region(&interval.first_yplane, &region) {
                timings.intervals = timings.intervals.saturating_add(1);
                continue;
            }

            let plane = LumaPlane::from_frame(&interval.first_yplane);
            let regions = [region];
            let request = OcrRequest::new(plane, &regions);
            let ocr_started = Instant::now();
            let response = self
                .engine
                .recognize(&request)
                .map_err(OcrStageError::Engine)?;
            timings.ocr_calls = timings.ocr_calls.saturating_add(1);
            timings.intervals = timings.intervals.saturating_add(1);
            timings.ocr_duration = timings.ocr_duration.saturating_add(ocr_started.elapsed());
            subtitles.push(OcredSubtitle {
                interval,
                region,
                response,
            });
        }

        timings.total = started.elapsed();

        Ok(OcrEvent {
            sample: event.sample,
            subtitles,
            segment_timings: event.segment_timings,
            timings: Some(timings),
        })
    }
}

fn roi_to_region(roi: &RoiConfig, frame: &YPlaneFrame) -> OcrRegion {
    let width = frame.width().max(1) as f32;
    let height = frame.height().max(1) as f32;
    let left = (roi.x * width).clamp(0.0, width);
    let top = (roi.y * height).clamp(0.0, height);
    let mut right = ((roi.x + roi.width) * width).clamp(left, width);
    let mut bottom = ((roi.y + roi.height) * height).clamp(top, height);
    let epsilon = 1e-3f32;
    if right >= width {
        right = (width - epsilon).max(left);
    }
    if bottom >= height {
        bottom = (height - epsilon).max(top);
    }
    OcrRegion {
        x: left,
        y: top,
        width: (right - left).max(1.0),
        height: (bottom - top).max(1.0),
    }
}

fn is_low_contrast_region(frame: &YPlaneFrame, region: &OcrRegion) -> bool {
    let frame_w = frame.width() as usize;
    let frame_h = frame.height() as usize;
    if frame_w == 0 || frame_h == 0 {
        return true;
    }

    let left = region.x.floor().clamp(0.0, frame_w as f32) as usize;
    let top = region.y.floor().clamp(0.0, frame_h as f32) as usize;
    let right = (region.x + region.width)
        .ceil()
        .clamp(left as f32, frame_w as f32) as usize;
    let bottom = (region.y + region.height)
        .ceil()
        .clamp(top as f32, frame_h as f32) as usize;

    let width = right.saturating_sub(left);
    let height = bottom.saturating_sub(top);
    if width == 0 || height == 0 {
        return true;
    }

    let stride = frame.stride();
    let data = frame.data();

    let step_x = std::cmp::max(1, width / 32);
    let step_y = std::cmp::max(1, height / 32);

    let mut min = 255u8;
    let mut max = 0u8;
    let mut sum: u64 = 0;
    let mut sum_sq: u64 = 0;
    let mut samples: u64 = 0;

    for y in (top..bottom).step_by(step_y) {
        let row = y.saturating_mul(stride);
        for x in (left..right).step_by(step_x) {
            let idx = row + x;
            if idx >= data.len() {
                break;
            }
            let value = data[idx];
            min = min.min(value);
            max = max.max(value);
            sum = sum.saturating_add(u64::from(value));
            sum_sq = sum_sq.saturating_add(u64::from(value) * u64::from(value));
            samples = samples.saturating_add(1);
        }
    }

    if samples == 0 {
        return true;
    }

    let range = max.saturating_sub(min);
    if range <= 3 {
        return true;
    }

    let mean = sum as f64 / samples as f64;
    let variance = (sum_sq as f64 / samples as f64) - mean * mean;
    let stddev = variance.max(0.0).sqrt();

    range <= 8 && stddev < 2.0
}

#[cfg(test)]
mod tests {
    use super::roi_to_region;
    use subtitle_fast_decoder::YPlaneFrame;
    use subtitle_fast_validator::subtitle_detection::RoiConfig;

    #[test]
    fn roi_to_region_clamps_to_bounds() {
        let frame = YPlaneFrame::from_owned(100, 50, 100, None, vec![0; 5000]).unwrap();
        let roi = RoiConfig {
            x: -0.2,
            y: 0.5,
            width: 1.4,
            height: 0.8,
        };
        let region = roi_to_region(&roi, &frame);
        assert_eq!(region.x, 0.0);
        assert!((region.y - 25.0).abs() < f32::EPSILON);
        assert_eq!(region.width, 100.0);
        assert_eq!(region.height, 25.0);
    }
}
