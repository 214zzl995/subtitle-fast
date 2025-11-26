use std::collections::BTreeMap;
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
const OCR_WORKER_CHANNEL_CAPACITY: usize = 2;
const OCR_MAX_WORKERS: usize = 1;

struct OcrJob {
    seq: u64,
    event: SegmenterEvent,
}

struct OrderedOcrResult {
    seq: u64,
    result: OcrStageResult,
}

fn ocr_worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|count| count.get().min(OCR_MAX_WORKERS))
        .unwrap_or(1)
        .max(1)
}

type RegionBounds = (usize, usize, usize, usize);
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
        let worker_count = ocr_worker_count();

        tokio::spawn(async move {
            if let Err(err) = engine.warm_up() {
                let _ = tx.send(Err(OcrStageError::Engine(err))).await;
                return;
            }

            let (result_tx, result_rx) =
                mpsc::channel::<OrderedOcrResult>(worker_count * OCR_CHANNEL_CAPACITY.max(1));
            let mut job_senders = Vec::with_capacity(worker_count);

            for _ in 0..worker_count {
                let (job_tx, mut job_rx) = mpsc::channel::<OcrJob>(OCR_WORKER_CHANNEL_CAPACITY);
                job_senders.push(job_tx);
                let worker = OcrWorker::new(Arc::clone(&engine));
                let result_tx = result_tx.clone();
                tokio::spawn(async move {
                    while let Some(job) = job_rx.recv().await {
                        let result = worker.handle_event(job.event);
                        let _ = result_tx
                            .send(OrderedOcrResult {
                                seq: job.seq,
                                result,
                            })
                            .await;
                    }
                });
            }

            let forward = tokio::spawn(async move {
                forward_ocr_results(result_rx, tx).await;
            });

            let mut upstream = stream;
            let mut seq: u64 = 0;
            let mut next_worker: usize = 0;
            let result_tx_main = result_tx;

            while let Some(event) = upstream.next().await {
                match event {
                    Ok(segment_event) => {
                        if job_senders.is_empty() {
                            break;
                        }
                        let job = OcrJob {
                            seq,
                            event: segment_event,
                        };
                        let sender = &job_senders[next_worker];
                        next_worker = (next_worker + 1) % job_senders.len();
                        if sender.send(job).await.is_err() {
                            break;
                        }
                        seq = seq.saturating_add(1);
                    }
                    Err(err) => {
                        let ordered = OrderedOcrResult {
                            seq,
                            result: Err(OcrStageError::Segmenter(err)),
                        };
                        let _ = result_tx_main.send(ordered).await;
                        break;
                    }
                }
            }

            drop(job_senders);
            drop(result_tx_main);

            let _ = forward.await;
        });

        let stream = Box::pin(unfold(rx, |mut receiver| async {
            receiver.recv().await.map(|item| (item, receiver))
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
    pub prefilter_runs: u64,
    pub prefilter_skips: u64,
    pub prefilter_duration: Duration,
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
            timings.intervals = timings.intervals.saturating_add(1);
            let region = roi_to_region(&interval.roi, &interval.first_yplane);
            let Some(bounds) = region_bounds(&region, &interval.first_yplane) else {
                continue;
            };

            let plane = LumaPlane::from_frame(&interval.first_yplane);
            let regions = [region];
            let request = OcrRequest::new(plane, &regions);
            let ocr_started = Instant::now();
            let response = match self.engine.recognize(&request) {
                Ok(resp) => resp,
                Err(err) => {
                    eprintln!(
                        "[ocr-error-debug] frame={} roi_norm=({:.3},{:.3},{:.3},{:.3}) region_px={}x{}@({},{}) error={}",
                        interval.start_frame,
                        interval.roi.x,
                        interval.roi.y,
                        interval.roi.width,
                        interval.roi.height,
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

async fn forward_ocr_results(
    mut results: mpsc::Receiver<OrderedOcrResult>,
    tx: mpsc::Sender<OcrStageResult>,
) {
    let mut next_seq: u64 = 0;
    let mut buffer: BTreeMap<u64, OcrStageResult> = BTreeMap::new();

    while let Some(OrderedOcrResult { seq, result }) = results.recv().await {
        buffer.insert(seq, result);
        while let Some(item) = buffer.remove(&next_seq) {
            if tx.send(item).await.is_err() {
                return;
            }
            next_seq = next_seq.saturating_add(1);
        }
    }

    while let Some(item) = buffer.remove(&next_seq) {
        if tx.send(item).await.is_err() {
            return;
        }
        next_seq = next_seq.saturating_add(1);
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

fn region_bounds(region: &OcrRegion, frame: &YPlaneFrame) -> Option<RegionBounds> {
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
        assert!((region.width - 100.0).abs() < 1e-3);
        assert!((region.height - 25.0).abs() < 1e-3);
    }
}
