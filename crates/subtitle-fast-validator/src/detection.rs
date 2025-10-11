use crate::config::{FrameMetadata, SubtitleDetectionOptions};
use crate::dump::FrameDumpOperation;
use crate::subtitle_detection::{
    build_detector, LumaBandConfig, SubtitleDetectionConfig, SubtitleDetectionError,
    SubtitleDetectionResult, SubtitleDetector,
};
use subtitle_fast_decoder::YPlaneFrame;
use tokio::sync::Mutex;

static REGION_MARGIN_PX: u32 = 5;

pub(crate) struct SubtitleDetectionPipeline {
    dump: Option<FrameDumpOperation>,
    state: Mutex<SubtitleDetectionState>,
    enabled: bool,
}

impl SubtitleDetectionPipeline {
    pub fn from_options(mut options: SubtitleDetectionOptions) -> Option<Self> {
        let dump_cfg = options.frame_dump.take();
        if !options.enabled && dump_cfg.is_none() {
            return None;
        }

        let dump = dump_cfg.map(FrameDumpOperation::new);
        let enabled = options.enabled;
        Some(Self {
            dump,
            enabled,
            state: Mutex::new(SubtitleDetectionState::new(options)),
        })
    }

    pub async fn process(
        &self,
        frame: &YPlaneFrame,
        metadata: &FrameMetadata,
    ) -> Option<SubtitleDetectionResult> {
        let mut detection = if self.enabled {
            let mut state = self.state.lock().await;
            state.process_frame(frame, metadata)
        } else {
            None
        };

        if let Some(result) = detection.as_mut() {
            inflate_regions(
                result,
                frame.width() as usize,
                frame.height() as usize,
                REGION_MARGIN_PX,
            );
        }

        if let Some(dump) = self.dump.as_ref() {
            if let Err(err) = dump.process(frame, metadata, detection.as_ref()).await {
                eprintln!("frame dump error: {err}");
            }
        }

        detection
    }

    pub async fn finalize(&self) {
        if let Some(dump) = self.dump.as_ref() {
            if let Err(err) = dump.finalize().await {
                eprintln!("frame dump finalize error: {err}");
            }
        }

        if !self.enabled {
            return;
        }

        let mut state = self.state.lock().await;
        state.finalize();
    }
}

struct SubtitleDetectionState {
    detector: Option<Box<dyn SubtitleDetector>>,
    detector_dims: Option<(usize, usize, usize)>,
    init_error_logged: bool,
    options: SubtitleDetectionOptions,
}

impl SubtitleDetectionState {
    fn new(options: SubtitleDetectionOptions) -> Self {
        Self {
            detector: None,
            detector_dims: None,
            init_error_logged: false,
            options,
        }
    }

    fn process_frame(
        &mut self,
        frame: &YPlaneFrame,
        metadata: &FrameMetadata,
    ) -> Option<SubtitleDetectionResult> {
        if !self.options.enabled {
            return None;
        }

        let dims = (
            frame.width() as usize,
            frame.height() as usize,
            frame.stride(),
        );
        if self.detector_dims != Some(dims) {
            self.detector_dims = Some(dims);
            let mut detector_config = SubtitleDetectionConfig::for_frame(dims.0, dims.1, dims.2);
            detector_config.model_path = self.options.onnx_model_path.clone();
            detector_config.luma_band = LumaBandConfig {
                target_luma: self.options.luma_band.target_luma,
                delta: self.options.luma_band.delta,
            };
            if let Some(roi) = self.options.roi_override {
                detector_config.roi = roi;
            }
            match build_detector(self.options.detector, detector_config) {
                Ok(detector) => {
                    self.detector = Some(detector);
                    self.init_error_logged = false;
                }
                Err(err) => {
                    if !self.init_error_logged {
                        log_init_failure(self.options.detector, err);
                        self.init_error_logged = true;
                    }
                    self.detector = None;
                    return None;
                }
            }
        }

        let Some(detector) = self.detector.as_ref() else {
            return None;
        };

        match detector.detect(frame.data(), metadata) {
            Ok(result) => Some(result),
            Err(err) => {
                eprintln!(
                    "subtitle detection failed for frame {}: {}",
                    metadata.frame_index, err
                );
                None
            }
        }
    }

    fn finalize(&mut self) {
        if !self.options.enabled {
            return;
        }
        self.detector = None;
        self.detector_dims = None;
    }
}

fn inflate_regions(
    result: &mut SubtitleDetectionResult,
    frame_width: usize,
    frame_height: usize,
    margin_px: u32,
) {
    if margin_px == 0 || frame_width == 0 || frame_height == 0 {
        return;
    }
    let margin = margin_px as f32;
    let frame_w = frame_width as f32;
    let frame_h = frame_height as f32;
    for region in &mut result.regions {
        let mut x0 = region.x - margin;
        let mut y0 = region.y - margin;
        let mut x1 = region.x + region.width + margin;
        let mut y1 = region.y + region.height + margin;

        if x0 < 0.0 {
            x0 = 0.0;
        }
        if y0 < 0.0 {
            y0 = 0.0;
        }
        if x1 > frame_w {
            x1 = frame_w;
        }
        if y1 > frame_h {
            y1 = frame_h;
        }

        region.x = x0;
        region.y = y0;
        region.width = (x1 - x0).max(0.0);
        region.height = (y1 - y0).max(0.0);
    }
}

fn log_init_failure(
    kind: crate::subtitle_detection::SubtitleDetectorKind,
    err: SubtitleDetectionError,
) {
    eprintln!(
        "subtitle detection initialization failed for backend '{}': {err}",
        kind.as_str()
    );
}
