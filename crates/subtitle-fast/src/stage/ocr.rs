use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream::unfold};
use tokio::sync::mpsc;

use super::StreamBundle;
use super::detector::DetectionSample;
use super::lifecycle::{
    CompletedRegion, LifecycleEvent, LifecycleResult, RegionLifecycleError, RegionTimings,
};
use subtitle_fast_ocr::{LumaPlane, OcrEngine, OcrError, OcrRequest};
use subtitle_fast_types::{OcrRegion, OcrResponse, RoiConfig, VideoFrame};

const OCR_CHANNEL_CAPACITY: usize = 4;

pub(crate) type RegionBounds = (usize, usize, usize, usize);
pub type OcrStageResult = Result<OcrEvent, OcrStageError>;

pub struct SubtitleOcr {
    engine: Arc<dyn OcrEngine>,
}

impl SubtitleOcr {
    pub fn new(engine: Arc<dyn OcrEngine>) -> Self {
        Self { engine }
    }

    pub fn attach(self, input: StreamBundle<LifecycleResult>) -> StreamBundle<OcrStageResult> {
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

            let worker = OcrWorker::new(Arc::clone(&engine));
            let mut upstream = stream;

            while let Some(event) = upstream.next().await {
                match event {
                    Ok(segment_event) => {
                        let result = worker.handle_event(segment_event);
                        let is_err = result.is_err();
                        if tx.send(result).await.is_err() {
                            return;
                        }
                        if is_err {
                            return;
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(Err(OcrStageError::Lifecycle(err))).await;
                        return;
                    }
                }
            }
        });

        let stream = Box::pin(unfold(rx, |mut receiver| async {
            receiver.recv().await.map(|item| (item, receiver))
        }));

        StreamBundle::new(stream, total_frames)
    }
}

pub struct OcrEvent {
    pub sample: Option<DetectionSample>,
    pub regions: Vec<OcredSubtitle>,
    pub region_timings: Option<RegionTimings>,
    pub timings: Option<OcrTimings>,
}

pub struct OcredSubtitle {
    pub lifecycle: CompletedRegion,
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
    Lifecycle(RegionLifecycleError),
    Engine(OcrError),
}

struct OcrWorker {
    engine: Arc<dyn OcrEngine>,
}

impl OcrWorker {
    fn new(engine: Arc<dyn OcrEngine>) -> Self {
        Self { engine }
    }

    fn handle_event(&self, event: LifecycleEvent) -> Result<OcrEvent, OcrStageError> {
        let started = Instant::now();
        let mut timings = OcrTimings::default();
        let mut subtitles = Vec::with_capacity(event.completed.len());

        for lifecycle in event.completed {
            timings.intervals = timings.intervals.saturating_add(1);
            let region = roi_to_region(&lifecycle.roi, &lifecycle.frame);
            let Some(bounds) = region_bounds(&region, &lifecycle.frame) else {
                continue;
            };

            let plane = LumaPlane::from_frame(&lifecycle.frame);
            let regions = [region];
            let request = OcrRequest::new(plane, &regions);
            let ocr_started = Instant::now();
            let response = match self.engine.recognize(&request) {
                Ok(resp) => resp,
                Err(err) => {
                    eprintln!(
                        "[ocr-error-debug] frame={} roi_norm=({:.3},{:.3},{:.3},{:.3}) region_px={}x{}@({},{}) error={}",
                        lifecycle.start_frame,
                        lifecycle.roi.x,
                        lifecycle.roi.y,
                        lifecycle.roi.width,
                        lifecycle.roi.height,
                        bounds.2.saturating_sub(bounds.0),
                        bounds.3.saturating_sub(bounds.1),
                        bounds.0,
                        bounds.1,
                        err,
                    );
                    return Err(OcrStageError::Engine(err));
                }
            };
            timings.ocr_calls = timings.ocr_calls.saturating_add(1);
            timings.ocr_duration = timings.ocr_duration.saturating_add(ocr_started.elapsed());
            subtitles.push(OcredSubtitle {
                lifecycle,
                region,
                response,
            });
        }

        timings.total = started.elapsed();

        Ok(OcrEvent {
            sample: event.sample,
            regions: subtitles,
            region_timings: event.region_timings,
            timings: Some(timings),
        })
    }
}

fn roi_to_region(roi: &RoiConfig, frame: &VideoFrame) -> OcrRegion {
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

pub(crate) fn region_bounds(region: &OcrRegion, frame: &VideoFrame) -> Option<RegionBounds> {
    let frame_w = frame.width() as usize;
    let frame_h = frame.height() as usize;
    if frame_w == 0 || frame_h == 0 {
        return None;
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
        return None;
    }

    Some((left, top, right, bottom))
}

#[cfg(test)]
mod tests {
    use super::roi_to_region;
    use subtitle_fast_types::{RoiConfig, VideoFrame};

    #[test]
    fn roi_to_region_clamps_to_bounds() {
        let frame =
            VideoFrame::from_nv12_owned(100, 50, 100, 100, None, vec![0; 5000], vec![128; 2500])
                .unwrap();
        let roi = RoiConfig {
            x: -0.2,
            y: 0.5,
            width: 1.4,
            height: 0.8,
        };
        let region = roi_to_region(&roi, &frame);
        assert_eq!(region.x, 0.0);
        assert!((region.y - 25.0).abs() < f32::EPSILON);
        assert!((region.width - 100.0).abs() < 1e-3);
        assert!((region.height - 25.0).abs() < 1e-3);
    }
}
