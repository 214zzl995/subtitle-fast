use crate::config::SubtitleDetectionOptions;
use crate::subtitle_detection::{
    build_detector, LumaBandConfig, RoiConfig, SubtitleDetectionConfig, SubtitleDetectionError,
    SubtitleDetectionResult, SubtitleDetector, SubtitleDetectorKind,
};
use std::time::Duration;
use subtitle_fast_decoder::YPlaneFrame;
use tokio::sync::Mutex;

static REGION_MARGIN_PX: u32 = 5;
pub(crate) struct SubtitleDetectionPipeline {
    state: Mutex<SubtitleDetectionState>,
    enabled: bool,
}

impl SubtitleDetectionPipeline {
    pub fn from_options(options: SubtitleDetectionOptions) -> Option<Self> {
        if !options.enabled {
            return None;
        }

        Some(Self {
            enabled: options.enabled,
            state: Mutex::new(SubtitleDetectionState::new(options)),
        })
    }

    pub async fn process(
        &self,
        frame: &YPlaneFrame,
        roi: Option<RoiConfig>,
    ) -> Result<SubtitleDetectionResult, SubtitleDetectionError> {
        let mut detection = if self.enabled {
            let mut state = self.state.lock().await;
            state.process_frame(frame, roi)?
        } else {
            SubtitleDetectionResult::empty()
        };

        if detection.has_subtitle {
            inflate_regions(
                &mut detection,
                frame.width() as usize,
                frame.height() as usize,
                REGION_MARGIN_PX,
            );
        }

        Ok(detection)
    }

    pub async fn finalize(&self) {
        if !self.enabled {
            return;
        }

        let mut state = self.state.lock().await;
        state.finalize();
    }
}

struct SubtitleDetectionState {
    detector: Option<Box<dyn SubtitleDetector>>,
    detector_kind: Option<SubtitleDetectorKind>,
    detector_dims: Option<(usize, usize, usize)>,
    detector_roi: Option<RoiConfig>,
    init_error_logged: bool,
    options: SubtitleDetectionOptions,
}

impl SubtitleDetectionState {
    fn new(options: SubtitleDetectionOptions) -> Self {
        Self {
            detector: None,
            detector_kind: None,
            detector_dims: None,
            detector_roi: None,
            init_error_logged: false,
            options,
        }
    }

    fn process_frame(
        &mut self,
        frame: &YPlaneFrame,
        roi_override: Option<RoiConfig>,
    ) -> Result<SubtitleDetectionResult, SubtitleDetectionError> {
        if !self.options.enabled {
            return Ok(SubtitleDetectionResult::empty());
        }

        let dims = (
            frame.width() as usize,
            frame.height() as usize,
            frame.stride(),
        );
        let desired_roi = roi_override.or(self.options.roi);
        let detector_kind = self.options.detector;
        let needs_rebuild = self.detector_dims != Some(dims)
            || self.detector_kind != Some(detector_kind)
            || self.detector_roi != desired_roi
            || self.detector.is_none();
        if needs_rebuild {
            self.detector_dims = Some(dims);
            self.detector_kind = Some(detector_kind);
            self.detector_roi = desired_roi;
            let mut detector_config = SubtitleDetectionConfig::for_frame(dims.0, dims.1, dims.2);
            detector_config.luma_band = LumaBandConfig {
                target_luma: self.options.luma_band.target_luma,
                delta: self.options.luma_band.delta,
            };
            if let Some(roi) = desired_roi {
                detector_config.roi = roi;
            }
            match build_detector(detector_kind, detector_config) {
                Ok(detector) => {
                    self.detector = Some(detector);
                    self.init_error_logged = false;
                }
                Err(err) => {
                    if !self.init_error_logged {
                        log_init_failure(detector_kind, &err);
                        self.init_error_logged = true;
                    }
                    self.detector = None;
                    self.detector_kind = None;
                    self.detector_dims = None;
                    self.detector_roi = None;
                    return Err(err);
                }
            }
        }

        let Some(detector) = self.detector.as_ref() else {
            return Ok(SubtitleDetectionResult::empty());
        };

        let frame_index = frame_identifier(frame);

        match detector.detect(frame) {
            Ok(result) => Ok(result),
            Err(err) => {
                eprintln!(
                    "subtitle detection failed for frame {}: {}",
                    frame_index, err
                );
                Err(err)
            }
        }
    }

    fn finalize(&mut self) {
        if !self.options.enabled {
            return;
        }
        self.detector = None;
        self.detector_kind = None;
        self.detector_dims = None;
        self.detector_roi = None;
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

fn frame_identifier(frame: &YPlaneFrame) -> u64 {
    frame
        .frame_index()
        .or_else(|| frame.timestamp().map(duration_millis))
        .unwrap_or_default()
}

fn duration_millis(duration: Duration) -> u64 {
    let millis = duration.as_millis();
    if millis > u64::MAX as u128 {
        u64::MAX
    } else {
        millis as u64
    }
}

fn log_init_failure(kind: SubtitleDetectorKind, err: &SubtitleDetectionError) {
    eprintln!(
        "subtitle detection initialization failed for backend '{}': {err}",
        kind.as_str()
    );
}
