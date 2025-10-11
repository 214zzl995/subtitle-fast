use crate::config::{FrameMetadata, SubtitleDetectionOptions};
use crate::dump::FrameDumpOperation;
use crate::subtitle_detection::{
    build_detector, LumaBandConfig, SubtitleDetectionConfig, SubtitleDetectionError,
    SubtitleDetector,
};
use subtitle_fast_decoder::YPlaneFrame;
use tokio::sync::Mutex;

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

    pub async fn process(&self, frame: &YPlaneFrame, metadata: &FrameMetadata) {
        if let Some(dump) = self.dump.as_ref() {
            if let Err(err) = dump.process(frame, metadata).await {
                eprintln!("frame dump error: {err}");
            }
        }

        if !self.enabled {
            return;
        }

        let mut state = self.state.lock().await;
        state.process_frame(frame, metadata);
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

    fn process_frame(&mut self, frame: &YPlaneFrame, metadata: &FrameMetadata) {
        if !self.options.enabled {
            return;
        }

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
                        log_init_failure(self.options.detector, err);
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
        if !self.options.enabled {
            return;
        }
        self.detector = None;
        self.detector_dims = None;
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
