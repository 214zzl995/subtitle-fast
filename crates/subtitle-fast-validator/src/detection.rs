use crate::config::SubtitleDetectionOptions;
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
    ) -> Result<SubtitleDetectionResult, SubtitleDetectionError> {
        let mut result = if self.enabled {
            let mut state = self.state.lock().await;
            state.process_frame(frame)?
        } else {
            SubtitleDetectionResult::empty()
        };

        inflate_regions(
            &mut result,
            frame.width() as usize,
            frame.height() as usize,
            REGION_MARGIN_PX,
        );

        if let Some(dump) = self.dump.as_ref() {
            dump.process(frame, &result)
                .await
                .map_err(SubtitleDetectionError::from)?;
        }

        Ok(result)
    }

    pub async fn finalize(&self) -> Result<(), SubtitleDetectionError> {
        if let Some(dump) = self.dump.as_ref() {
            dump.finalize()
                .await
                .map_err(SubtitleDetectionError::from)?;
        }

        if !self.enabled {
            return Ok(());
        }

        let mut state = self.state.lock().await;
        state.finalize();
        Ok(())
    }
}

struct SubtitleDetectionState {
    detector: Option<Box<dyn SubtitleDetector>>,
    detector_dims: Option<(usize, usize, usize)>,
    options: SubtitleDetectionOptions,
}

impl SubtitleDetectionState {
    fn new(options: SubtitleDetectionOptions) -> Self {
        Self {
            detector: None,
            detector_dims: None,
            options,
        }
    }

    fn process_frame(
        &mut self,
        frame: &YPlaneFrame,
    ) -> Result<SubtitleDetectionResult, SubtitleDetectionError> {
        if !self.options.enabled {
            return Ok(SubtitleDetectionResult::empty());
        }

        let dims = (
            frame.width() as usize,
            frame.height() as usize,
            frame.stride(),
        );

        if self.detector_dims != Some(dims) {
            let mut detector_config = SubtitleDetectionConfig::for_frame(dims.0, dims.1, dims.2);
            detector_config.model_path = self.options.onnx_model_path.clone();
            if let Some(roi) = self.options.roi {
                detector_config.roi = roi;
            }
            detector_config.luma_band = LumaBandConfig {
                target_luma: self.options.luma_band.target_luma,
                delta: self.options.luma_band.delta,
            };
            let detector = build_detector(self.options.detector, detector_config)?;
            self.detector = Some(detector);
            self.detector_dims = Some(dims);
        }

        let Some(detector) = self.detector.as_ref() else {
            return Ok(SubtitleDetectionResult::empty());
        };

        let result = detector.detect(frame.data())?;
        Ok(result)
    }

    fn finalize(&mut self) {
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
