use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use futures_util::{StreamExt, stream::unfold};
use parking_lot::Mutex;
use tokio::sync::mpsc;

use super::StreamBundle;
use super::detector::{DetectionSample, DetectionSampleResult, DetectorError};
use subtitle_fast_types::{DetectionRegion, PlaneFrame, RoiConfig};

const REGION_DETERMINER_CHANNEL_CAPACITY: usize = 4;
const IOU_THRESHOLD: f32 = 0.05;

pub type RegionId = u64;

pub struct RegionUnit {
    pub id: RegionId,
    pub label: String,
    pub roi: RoiConfig,
}

pub struct RegionDeterminerEvent {
    pub sample: DetectionSample,
    pub regions: Vec<RegionUnit>,
}

pub type RegionDeterminerResult = Result<RegionDeterminerEvent, RegionDeterminerError>;

#[derive(Debug)]
pub enum RegionDeterminerError {
    Detector(DetectorError),
}

pub struct RegionDeterminer {
    persistent: Arc<Mutex<PersistentStore>>,
}

impl RegionDeterminer {
    pub fn new() -> Self {
        Self {
            persistent: Arc::new(Mutex::new(PersistentStore::new())),
        }
    }

    pub fn attach(
        self,
        input: StreamBundle<DetectionSampleResult>,
    ) -> StreamBundle<RegionDeterminerResult> {
        let StreamBundle {
            stream,
            total_frames,
        } = input;

        let persistent = Arc::clone(&self.persistent);
        let (tx, rx) = mpsc::channel::<RegionDeterminerResult>(REGION_DETERMINER_CHANNEL_CAPACITY);

        tokio::spawn(async move {
            let mut upstream = stream;
            let mut worker = RegionDeterminerWorker::new(persistent);

            while let Some(event) = upstream.next().await {
                match event {
                    Ok(sample) => {
                        let result = worker.handle_sample(sample);
                        if tx.send(Ok(result)).await.is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(Err(RegionDeterminerError::Detector(err))).await;
                        break;
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

impl Default for RegionDeterminer {
    fn default() -> Self {
        Self::new()
    }
}

struct RegionDeterminerWorker {
    persistent: Arc<Mutex<PersistentStore>>,
}

impl RegionDeterminerWorker {
    fn new(persistent: Arc<Mutex<PersistentStore>>) -> Self {
        Self { persistent }
    }

    fn handle_sample(&mut self, sample: DetectionSample) -> RegionDeterminerEvent {
        let yplane = sample.sample.frame_handle();
        let mut used_ids = HashSet::new();
        let mut emitted: Vec<RegionUnit> = Vec::with_capacity(sample.detection.regions.len());

        for region in &sample.detection.regions {
            let roi = region_to_roi(region, &yplane);
            let matched = {
                let store = self.persistent.lock();
                store.best_match(&roi, &used_ids)
            };

            let (id, label, previous_roi) = if let Some(region) = matched {
                let mut guard = region.lock();
                used_ids.insert(guard.id);
                let prev_roi = guard.roi;
                guard.roi = roi;
                (guard.id, guard.label.clone(), Some(prev_roi))
            } else {
                let created = {
                    let mut store = self.persistent.lock();
                    store.insert_new(roi)
                };
                let guard = created.lock();
                used_ids.insert(guard.id);
                (guard.id, guard.label.clone(), None)
            };

            if let Some(previous) = previous_roi
                && roi_area(&roi) > roi_area(&previous)
                && overlaps(&roi, &previous)
                && let Some(clipped) = clip_region(&previous, &roi)
            {
                let created = {
                    let mut store = self.persistent.lock();
                    store.insert_new(clipped)
                };
                let guard = created.lock();
                used_ids.insert(guard.id);
                emitted.push(RegionUnit {
                    id: guard.id,
                    label: guard.label.clone(),
                    roi: clipped,
                });
            }

            emitted.push(RegionUnit { id, label, roi });
        }

        RegionDeterminerEvent {
            sample,
            regions: emitted,
        }
    }
}

struct PersistentStore {
    regions: HashMap<RegionId, Arc<Mutex<PersistentRegion>>>,
    next_id: RegionId,
}

impl PersistentStore {
    fn new() -> Self {
        Self {
            regions: HashMap::new(),
            next_id: 0,
        }
    }

    fn insert_new(&mut self, roi: RoiConfig) -> Arc<Mutex<PersistentRegion>> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let label = format!("region-{id}");
        let region = Arc::new(Mutex::new(PersistentRegion { id, label, roi }));
        self.regions.insert(id, Arc::clone(&region));
        region
    }

    fn best_match(
        &self,
        roi: &RoiConfig,
        used_ids: &HashSet<RegionId>,
    ) -> Option<Arc<Mutex<PersistentRegion>>> {
        let mut best_id = None;
        let mut best_iou = 0.0_f32;
        for (id, region) in self.regions.iter() {
            if used_ids.contains(id) {
                continue;
            }
            let guard = region.lock();
            let iou = roi_iou(roi, &guard.roi);
            if iou > best_iou && iou >= IOU_THRESHOLD {
                best_iou = iou;
                best_id = Some(*id);
            }
        }
        best_id.and_then(|id| self.regions.get(&id).cloned())
    }
}

struct PersistentRegion {
    id: RegionId,
    label: String,
    roi: RoiConfig,
}

fn region_to_roi(region: &DetectionRegion, frame: &PlaneFrame) -> RoiConfig {
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

fn overlaps(a: &RoiConfig, b: &RoiConfig) -> bool {
    let Some(inter) = roi_intersection(a, b) else {
        return false;
    };
    inter.width > 0.0 && inter.height > 0.0
}

fn roi_area(roi: &RoiConfig) -> f32 {
    (roi.width.max(0.0)) * (roi.height.max(0.0))
}

fn roi_iou(a: &RoiConfig, b: &RoiConfig) -> f32 {
    let Some(inter) = roi_intersection(a, b) else {
        return 0.0;
    };
    let inter_area = roi_area(&inter);
    let union = roi_area(a) + roi_area(b) - inter_area;
    if union <= 0.0 {
        return 0.0;
    }
    inter_area / union
}

fn roi_intersection(a: &RoiConfig, b: &RoiConfig) -> Option<RoiConfig> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.width).min(b.x + b.width);
    let y1 = (a.y + a.height).min(b.y + b.height);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(RoiConfig {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    })
}

fn clip_region(smaller: &RoiConfig, larger: &RoiConfig) -> Option<RoiConfig> {
    let inter = roi_intersection(smaller, larger)?;

    let candidates = [
        // Above overlap
        RoiConfig {
            x: smaller.x,
            y: smaller.y,
            width: smaller.width,
            height: (inter.y - smaller.y).max(0.0),
        },
        // Below overlap
        RoiConfig {
            x: smaller.x,
            y: (inter.y + inter.height).min(smaller.y + smaller.height),
            width: smaller.width,
            height: (smaller.y + smaller.height - (inter.y + inter.height)).max(0.0),
        },
        // Left of overlap
        RoiConfig {
            x: smaller.x,
            y: smaller.y,
            width: (inter.x - smaller.x).max(0.0),
            height: smaller.height,
        },
        // Right of overlap
        RoiConfig {
            x: (inter.x + inter.width).min(smaller.x + smaller.width),
            y: smaller.y,
            width: (smaller.x + smaller.width - (inter.x + inter.width)).max(0.0),
            height: smaller.height,
        },
    ];

    candidates
        .into_iter()
        .filter(|roi| roi.width > 0.0 && roi.height > 0.0)
        .max_by(|a, b| {
            roi_area(a)
                .partial_cmp(&roi_area(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}
