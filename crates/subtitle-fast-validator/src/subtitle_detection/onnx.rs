use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ndarray::{Array4, CowArray, IxDyn};
use once_cell::sync::OnceCell;
use ort::environment::Environment;
use ort::error::OrtError;
use ort::session::{Session, SessionBuilder};
use ort::value::Value;

use super::{
    DetectionRegion, RoiConfig, SubtitleDetectionConfig, SubtitleDetectionError,
    SubtitleDetectionResult, SubtitleDetector,
};
use subtitle_fast_decoder::YPlaneFrame;

const MODEL_INPUT_WIDTH: usize = 640;
const MODEL_INPUT_HEIGHT: usize = 640;

#[derive(Debug, Clone, Copy)]
struct RoiRect {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

#[derive(Debug, Clone)]
struct ModelHandle {
    _environment: Arc<Environment>,
    session: Arc<Session>,
}

struct ModelRegistry {
    environment: Arc<Environment>,
    handles: Mutex<HashMap<PathBuf, Arc<ModelHandle>>>,
}

impl ModelRegistry {
    fn new() -> Result<Self, SubtitleDetectionError> {
        let environment = Environment::builder()
            .with_name("subtitle-fast-validator")
            .build()
            .map_err(map_environment_error)?;
        Ok(Self {
            environment: Arc::new(environment),
            handles: Mutex::new(HashMap::new()),
        })
    }

    fn get(&self, path: &Path) -> Result<Arc<ModelHandle>, SubtitleDetectionError> {
        if !path.exists() {
            return Err(SubtitleDetectionError::ModelNotFound {
                path: path.to_path_buf(),
            });
        }

        let mut handles = self.handles.lock().expect("model registry poisoned");
        if let Some(handle) = handles.get(path) {
            return Ok(handle.clone());
        }

        let session = SessionBuilder::new(&self.environment)
            .map_err(map_session_error)?
            .with_model_from_file(path)
            .map_err(map_session_error)?;

        let handle = Arc::new(ModelHandle {
            _environment: Arc::clone(&self.environment),
            session: Arc::new(session),
        });
        handles.insert(path.to_path_buf(), handle.clone());
        Ok(handle)
    }
}

static MODEL_REGISTRY: OnceCell<ModelRegistry> = OnceCell::new();

fn registry() -> Result<&'static ModelRegistry, SubtitleDetectionError> {
    MODEL_REGISTRY.get_or_try_init(ModelRegistry::new)
}

pub fn ensure_model_ready(model_path: Option<&Path>) -> Result<(), SubtitleDetectionError> {
    let path = model_path
        .map(Path::to_path_buf)
        .ok_or(SubtitleDetectionError::MissingOnnxModelPath)?;
    let registry = registry()?;
    registry.get(&path)?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct OnnxPpocrDetector {
    config: SubtitleDetectionConfig,
    model: Arc<ModelHandle>,
}

impl OnnxPpocrDetector {
    pub fn new(config: SubtitleDetectionConfig) -> Result<Self, SubtitleDetectionError> {
        let required = config
            .stride
            .checked_mul(config.frame_height)
            .unwrap_or(usize::MAX);
        if required == usize::MAX {
            return Err(SubtitleDetectionError::InsufficientData {
                data_len: 0,
                required,
            });
        }
        let roi = compute_roi_rect(config.frame_width, config.frame_height, config.roi)?;
        if roi.height == 0 || roi.width == 0 {
            return Err(SubtitleDetectionError::EmptyRoi);
        }

        let model_path = config
            .model_path
            .as_ref()
            .ok_or(SubtitleDetectionError::MissingOnnxModelPath)?
            .clone();
        let model = registry()?.get(&model_path)?;

        Ok(Self { config, model })
    }
}

impl SubtitleDetector for OnnxPpocrDetector {
    fn ensure_available(config: &SubtitleDetectionConfig) -> Result<(), SubtitleDetectionError> {
        ensure_model_ready(config.model_path.as_deref())
    }

    fn detect(
        &self,
        frame: &YPlaneFrame,
    ) -> Result<SubtitleDetectionResult, SubtitleDetectionError> {
        let y_plane = frame.data();
        let required = self
            .config
            .stride
            .checked_mul(self.config.frame_height)
            .unwrap_or(usize::MAX);
        if y_plane.len() < required {
            return Err(SubtitleDetectionError::InsufficientData {
                data_len: y_plane.len(),
                required,
            });
        }

        let roi = compute_roi_rect(
            self.config.frame_width,
            self.config.frame_height,
            self.config.roi,
        )?;
        let roi_data = extract_roi(y_plane, self.config.stride, roi);
        let resized = resize_bilinear_to_model(
            &roi_data,
            roi.width,
            roi.height,
            MODEL_INPUT_WIDTH,
            MODEL_INPUT_HEIGHT,
        );
        let input = prepare_input_tensor(&resized)?;
        let (output, shape) = run_model(self.model.as_ref(), &input)?;
        let (regions, max_score) =
            decode_regions(&output, &shape, MODEL_INPUT_WIDTH, MODEL_INPUT_HEIGHT, roi);
        let has_subtitle = !regions.is_empty();
        let result = SubtitleDetectionResult {
            has_subtitle,
            max_score,
            regions,
        };

        Ok(result)
    }
}

fn map_environment_error(err: OrtError) -> SubtitleDetectionError {
    map_schema_conflict(err, SubtitleDetectionError::Environment)
}

fn map_session_error(err: OrtError) -> SubtitleDetectionError {
    map_schema_conflict(err, SubtitleDetectionError::Session)
}

fn map_schema_conflict<F>(err: OrtError, default: F) -> SubtitleDetectionError
where
    F: FnOnce(String) -> SubtitleDetectionError,
{
    let message = err.to_string();
    if message.contains("Trying to register schema with name") {
        SubtitleDetectionError::RuntimeSchemaConflict { message }
    } else {
        default(message)
    }
}

fn compute_roi_rect(
    frame_width: usize,
    frame_height: usize,
    roi: RoiConfig,
) -> Result<RoiRect, SubtitleDetectionError> {
    let start_x = (roi.x * frame_width as f32).round() as isize;
    let start_y = (roi.y * frame_height as f32).round() as isize;
    let end_x = ((roi.x + roi.width) * frame_width as f32).round() as isize;
    let end_y = ((roi.y + roi.height) * frame_height as f32).round() as isize;

    let start_x = start_x.clamp(0, frame_width as isize);
    let start_y = start_y.clamp(0, frame_height as isize);
    let end_x = end_x.clamp(start_x, frame_width as isize);
    let end_y = end_y.clamp(start_y, frame_height as isize);

    let width = (end_x - start_x) as usize;
    let height = (end_y - start_y) as usize;
    if width == 0 || height == 0 {
        return Err(SubtitleDetectionError::EmptyRoi);
    }

    Ok(RoiRect {
        x: start_x as usize,
        y: start_y as usize,
        width,
        height,
    })
}

fn extract_roi(data: &[u8], stride: usize, roi: RoiRect) -> Vec<u8> {
    let mut out = Vec::with_capacity(roi.width * roi.height);
    for row in 0..roi.height {
        let src_start = (roi.y + row) * stride + roi.x;
        out.extend_from_slice(&data[src_start..src_start + roi.width]);
    }
    out
}

fn resize_bilinear_to_model(
    src: &[u8],
    src_width: usize,
    src_height: usize,
    dst_width: usize,
    dst_height: usize,
) -> Vec<f32> {
    if src_width == 0 || src_height == 0 || dst_width == 0 || dst_height == 0 {
        return vec![0.0; dst_width * dst_height];
    }

    let mut out = vec![0.0f32; dst_width * dst_height];
    let scale_x = if dst_width > 1 {
        (src_width - 1) as f32 / (dst_width - 1) as f32
    } else {
        0.0
    };
    let scale_y = if dst_height > 1 {
        (src_height - 1) as f32 / (dst_height - 1) as f32
    } else {
        0.0
    };

    for dy in 0..dst_height {
        let fy = scale_y * dy as f32;
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(src_height - 1);
        let wy = fy - y0 as f32;
        for dx in 0..dst_width {
            let fx = scale_x * dx as f32;
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(src_width - 1);
            let wx = fx - x0 as f32;

            let top_left = src[y0 * src_width + x0] as f32;
            let top_right = src[y0 * src_width + x1] as f32;
            let bottom_left = src[y1 * src_width + x0] as f32;
            let bottom_right = src[y1 * src_width + x1] as f32;

            let top = top_left + (top_right - top_left) * wx;
            let bottom = bottom_left + (bottom_right - bottom_left) * wx;
            let value = top + (bottom - top) * wy;
            out[dy * dst_width + dx] = value;
        }
    }
    out
}

fn prepare_input_tensor(resized: &[f32]) -> Result<Array4<f32>, SubtitleDetectionError> {
    let area = MODEL_INPUT_WIDTH * MODEL_INPUT_HEIGHT;
    if resized.len() != area {
        return Err(SubtitleDetectionError::Input(
            "resized roi has unexpected size".to_string(),
        ));
    }
    let mut data = vec![0f32; area * 3];
    for i in 0..area {
        let value = (resized[i] / 255.0).clamp(0.0, 1.0);
        data[i] = value;
        data[i + area] = value;
        data[i + 2 * area] = value;
    }
    Array4::from_shape_vec((1, 3, MODEL_INPUT_HEIGHT, MODEL_INPUT_WIDTH), data)
        .map_err(|err| SubtitleDetectionError::Input(err.to_string()))
}

fn run_model(
    model: &ModelHandle,
    input: &Array4<f32>,
) -> Result<(Vec<f32>, Vec<usize>), SubtitleDetectionError> {
    let session = &model.session;
    let allocator = session.allocator();
    let input_dyn: CowArray<'_, f32, IxDyn> = CowArray::from(input.view().into_dyn());
    let value = Value::from_array(allocator, &input_dyn)
        .map_err(|err| SubtitleDetectionError::Input(err.to_string()))?;
    let outputs = session
        .run(vec![value])
        .map_err(|err| SubtitleDetectionError::Inference(err.to_string()))?;
    let tensor = outputs
        .into_iter()
        .next()
        .ok_or(SubtitleDetectionError::InvalidOutputShape)?
        .try_extract::<f32>()
        .map_err(|err| SubtitleDetectionError::Inference(err.to_string()))?;
    let view = tensor.view();
    let shape = view.shape().to_vec();
    let data = view.iter().copied().collect::<Vec<f32>>();
    Ok((data, shape))
}

fn decode_regions(
    data: &[f32],
    shape: &[usize],
    width: usize,
    height: usize,
    roi: RoiRect,
) -> (Vec<DetectionRegion>, f32) {
    let (map_height, map_width) = match shape {
        [h, w] => (*h, *w),
        [1, 1, h, w] => (*h, *w),
        [1, h, w] => (*h, *w),
        _ => (height, width),
    };
    let area = map_height.saturating_mul(map_width);
    let map = if area == 0 {
        Vec::new()
    } else if area <= data.len() {
        data[..area].to_vec()
    } else {
        data.to_vec()
    };
    let max_score = map.iter().copied().fold(0.0f32, f32::max);
    let threshold = 0.3f32;
    let mut regions = Vec::new();

    if map_height == 0 || map_width == 0 || map.len() < map_height * map_width {
        return (regions, max_score);
    }

    let mut visited = vec![false; map_height * map_width];
    for y in 0..map_height {
        for x in 0..map_width {
            let idx = y * map_width + x;
            if visited[idx] || map[idx] < threshold {
                continue;
            }
            let mut stack = vec![(x, y)];
            let mut min_x = x;
            let mut max_x = x;
            let mut min_y = y;
            let mut max_y = y;
            let mut sum = 0.0f32;
            let mut count = 0usize;

            while let Some((cx, cy)) = stack.pop() {
                let cidx = cy * map_width + cx;
                if visited[cidx] || map[cidx] < threshold {
                    continue;
                }
                visited[cidx] = true;
                sum += map[cidx];
                count += 1;
                min_x = min_x.min(cx);
                max_x = max_x.max(cx);
                min_y = min_y.min(cy);
                max_y = max_y.max(cy);

                let neighbors = [
                    (cx.wrapping_sub(1), cy),
                    (cx + 1, cy),
                    (cx, cy.wrapping_sub(1)),
                    (cx, cy + 1),
                ];
                for (nx, ny) in neighbors {
                    if nx < map_width && ny < map_height {
                        let nidx = ny * map_width + nx;
                        if !visited[nidx] && map[nidx] >= threshold {
                            stack.push((nx, ny));
                        }
                    }
                }
            }

            if count == 0 {
                continue;
            }
            let avg_score = sum / count as f32;

            let scale_x = roi.width as f32 / map_width as f32;
            let scale_y = roi.height as f32 / map_height as f32;
            let region = DetectionRegion {
                x: roi.x as f32 + min_x as f32 * scale_x,
                y: roi.y as f32 + min_y as f32 * scale_y,
                width: (max_x - min_x + 1) as f32 * scale_x,
                height: (max_y - min_y + 1) as f32 * scale_y,
                score: avg_score,
            };
            regions.push(region);
        }
    }

    (regions, max_score)
}
