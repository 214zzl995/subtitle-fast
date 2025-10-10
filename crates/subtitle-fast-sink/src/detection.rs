use crate::config::{FrameMetadata, SubtitleDetectionOptions};
use crate::subtitle_detection::{
    build_detector, LumaBandConfig, SubtitleDetectionConfig, SubtitleDetector,
};
use subtitle_fast_decoder::YPlaneFrame;
use tokio::sync::Mutex;

pub(crate) struct SubtitleDetectionOperation {
    state: Mutex<SubtitleDetectionState>,
}

impl SubtitleDetectionOperation {
    pub fn new(options: SubtitleDetectionOptions) -> Self {
        Self {
            state: Mutex::new(SubtitleDetectionState::new(options)),
        }
    }

    pub async fn process(&self, frame: &YPlaneFrame, metadata: &FrameMetadata) {
        let mut state = self.state.lock().await;
        state.process_frame(frame, metadata);
    }

    pub async fn finalize(&self) {
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

    fn process_frame(&mut self, frame: &YPlaneFrame, metadata: &FrameMetadata) {
        let dims = (
            frame.width() as usize,
            frame.height() as usize,
            frame.stride(),
        );
        if self.detector_dims != Some(dims) {
            self.detector_dims = Some(dims);
            let mut detector_config = SubtitleDetectionConfig::for_frame(dims.0, dims.1, dims.2);
            detector_config.dump_json = self.options.dump_json;
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
                        eprintln!("subtitle detection initialization failed: {err}");
                        self.init_error_logged = true;
                    }
                    self.detector = None;
                    return;
                }
            }
        }

        let Some(detector) = self.detector.as_ref() else {
            return;
        };

        if let Err(err) = detector.detect(frame.data(), metadata) {
            eprintln!(
                "subtitle detection failed for frame {}: {}",
                metadata.frame_index, err
            );
        }
    }

    fn finalize(&mut self) {
        self.detector = None;
        self.detector_dims = None;
    }
}
