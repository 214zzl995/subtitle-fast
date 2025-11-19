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
