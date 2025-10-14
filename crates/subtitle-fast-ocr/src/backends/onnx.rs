use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ndarray::{Array4, CowArray, IxDyn};
use once_cell::sync::OnceCell;
use ort::environment::Environment;
use ort::error::OrtError;
use ort::session::{Session, SessionBuilder};
use ort::value::Value;

use crate::plane::LumaPlane;
use crate::{OcrEngine, OcrError, OcrRegion, OcrRequest, OcrResponse, OcrText};

const INPUT_HEIGHT: usize = 48;
const INPUT_WIDTH: usize = 320;

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
    fn new() -> Result<Self, OcrError> {
        let environment = Environment::builder()
            .with_name("subtitle-fast-ocr")
            .build()
            .map_err(map_environment_error)?;
        Ok(Self {
            environment: Arc::new(environment),
            handles: Mutex::new(HashMap::new()),
        })
    }

    fn get(&self, path: &Path) -> Result<Arc<ModelHandle>, OcrError> {
        if !path.exists() {
            return Err(OcrError::backend(format!(
                "onnx model file '{}' does not exist",
                path.display()
            )));
        }

        let mut guard = self.handles.lock().expect("onnx registry poisoned");
        if let Some(handle) = guard.get(path) {
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
        guard.insert(path.to_path_buf(), handle.clone());
        Ok(handle)
    }
}

static MODEL_REGISTRY: OnceCell<ModelRegistry> = OnceCell::new();

fn registry() -> Result<&'static ModelRegistry, OcrError> {
    MODEL_REGISTRY.get_or_try_init(ModelRegistry::new)
}

#[derive(Debug)]
pub struct OnnxOcrEngine {
    model: Arc<ModelHandle>,
    alphabet: Arc<Vec<char>>,
}

impl OnnxOcrEngine {
    pub fn new(model_path: PathBuf) -> Result<Self, OcrError> {
        let registry = registry()?;
        let model = registry.get(&model_path)?;
        Ok(Self {
            model,
            alphabet: Arc::new(default_alphabet()),
        })
    }

    fn recognize_region(
        &self,
        plane: &LumaPlane<'_>,
        region: &OcrRegion,
    ) -> Result<Option<(String, Option<f32>)>, OcrError> {
        let (x, y, width, height) = match clamp_region(region, plane.width(), plane.height()) {
            Some(bounds) => bounds,
            None => return Ok(None),
        };
        let roi = extract_region(plane, x, y, width, height);
        if roi.is_empty() {
            return Ok(None);
        }

        let resized = resize_with_padding(&roi, width, height, INPUT_WIDTH, INPUT_HEIGHT);
        let input = prepare_input_tensor(&resized, INPUT_WIDTH, INPUT_HEIGHT)?;
        let (data, shape) = self.run_model(&input)?;
        let (text, confidence) = decode_sequence(&data, &shape, &self.alphabet)?;
        if text.trim().is_empty() {
            Ok(None)
        } else {
            Ok(Some((text, confidence)))
        }
    }

    fn run_model(&self, input: &Array4<f32>) -> Result<(Vec<f32>, Vec<usize>), OcrError> {
        let session = &self.model.session;
        let allocator = session.allocator();
        let input_dyn: CowArray<'_, f32, IxDyn> = CowArray::from(input.view().into_dyn());
        let tensor = Value::from_array(allocator, &input_dyn).map_err(map_input_error)?;
        let outputs = session.run(vec![tensor]).map_err(map_inference_error)?;
        let tensor = outputs
            .into_iter()
            .next()
            .ok_or_else(|| OcrError::backend("onnx model produced no output"))?
            .try_extract::<f32>()
            .map_err(map_inference_error)?;
        let view = tensor.view();
        let shape = view.shape().to_vec();
        let data = view.iter().copied().collect::<Vec<f32>>();
        Ok((data, shape))
    }
}

impl OcrEngine for OnnxOcrEngine {
    fn name(&self) -> &'static str {
        "onnx_ocr"
    }

    fn recognize(&self, request: &OcrRequest<'_>) -> Result<OcrResponse, OcrError> {
        let plane = request.plane();
        let mut texts = Vec::new();
        for region in request.regions() {
            if let Some((text, confidence)) = self.recognize_region(plane, region)? {
                let mut entry = OcrText::new(*region, text);
                if let Some(conf) = confidence {
                    entry = entry.with_confidence(conf);
                }
                texts.push(entry);
            }
        }
        Ok(OcrResponse::new(texts))
    }
}

fn map_environment_error(err: OrtError) -> OcrError {
    map_schema_conflict(err, "failed to initialise ONNX runtime environment")
}

fn map_session_error(err: OrtError) -> OcrError {
    map_schema_conflict(err, "failed to load ONNX model")
}

fn map_input_error(err: OrtError) -> OcrError {
    OcrError::backend(format!("failed to prepare ONNX input: {err}"))
}

fn map_inference_error(err: OrtError) -> OcrError {
    OcrError::backend(format!("ONNX inference failed: {err}"))
}

fn map_schema_conflict(err: OrtError, context: &str) -> OcrError {
    let message = err.to_string();
    if message.contains("Trying to register schema with name") {
        OcrError::backend(format!(
            "{context}: detected ONNX Runtime schema registration conflict ({message})"
        ))
    } else {
        OcrError::backend(format!("{context}: {message}"))
    }
}

fn clamp_region(
    region: &OcrRegion,
    width: u32,
    height: u32,
) -> Option<(usize, usize, usize, usize)> {
    let max_w = width as f32;
    let max_h = height as f32;
    if region.width <= 0.0 || region.height <= 0.0 {
        return None;
    }
    let x0 = region.x.clamp(0.0, max_w);
    let y0 = region.y.clamp(0.0, max_h);
    let x1 = (region.x + region.width).clamp(0.0, max_w);
    let y1 = (region.y + region.height).clamp(0.0, max_h);
    let x_start = x0.floor() as i32;
    let y_start = y0.floor() as i32;
    let x_end = x1.ceil() as i32;
    let y_end = y1.ceil() as i32;
    let width = (x_end - x_start).max(0) as usize;
    let height = (y_end - y_start).max(0) as usize;
    if width == 0 || height == 0 {
        return None;
    }
    Some((
        x_start.max(0) as usize,
        y_start.max(0) as usize,
        width,
        height,
    ))
}

fn extract_region(
    plane: &LumaPlane<'_>,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) -> Vec<u8> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let stride = plane.stride();
    let data = plane.data();
    let mut out = Vec::with_capacity(width * height);
    for row in 0..height {
        let start = (y + row) * stride + x;
        out.extend_from_slice(&data[start..start + width]);
    }
    out
}

fn resize_with_padding(
    src: &[u8],
    src_width: usize,
    src_height: usize,
    dst_width: usize,
    dst_height: usize,
) -> Vec<f32> {
    if src_width == 0 || src_height == 0 || dst_width == 0 || dst_height == 0 {
        return vec![0.0; dst_width * dst_height];
    }
    let mut scaled_width =
        ((dst_height as f32 / src_height as f32) * src_width as f32).round() as usize;
    scaled_width = scaled_width.clamp(1, dst_width);
    let resized = resize_bilinear(src, src_width, src_height, scaled_width, dst_height);
    let mut canvas = vec![0.0f32; dst_width * dst_height];
    for row in 0..dst_height {
        let dst_row = &mut canvas[row * dst_width..(row + 1) * dst_width];
        let src_row = &resized[row * scaled_width..(row + 1) * scaled_width];
        dst_row[..scaled_width].copy_from_slice(src_row);
    }
    canvas
}

fn resize_bilinear(
    src: &[u8],
    src_width: usize,
    src_height: usize,
    dst_width: usize,
    dst_height: usize,
) -> Vec<f32> {
    if dst_width == 0 || dst_height == 0 {
        return Vec::new();
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
            out[dy * dst_width + dx] = (value / 255.0).clamp(0.0, 1.0);
        }
    }
    out
}

fn prepare_input_tensor(
    normalized: &[f32],
    width: usize,
    height: usize,
) -> Result<Array4<f32>, OcrError> {
    if normalized.len() != width * height {
        return Err(OcrError::backend(
            "normalized image has unexpected length for ONNX input",
        ));
    }
    let mut data = vec![0.0f32; normalized.len() * 3];
    let area = width * height;
    for i in 0..area {
        let value = normalized[i];
        data[i] = value;
        data[i + area] = value;
        data[i + 2 * area] = value;
    }
    Array4::from_shape_vec((1, 3, height, width), data)
        .map_err(|err| OcrError::backend(format!("failed to build ONNX input tensor: {err}")))
}

fn decode_sequence(
    data: &[f32],
    shape: &[usize],
    alphabet: &[char],
) -> Result<(String, Option<f32>), OcrError> {
    let mut dims: Vec<usize> = shape.iter().copied().collect();
    while dims.len() > 2 && dims.first() == Some(&1) {
        dims.remove(0);
    }
    while dims.len() > 2 && dims.last() == Some(&1) {
        dims.pop();
    }
    if dims.len() > 2 {
        return Err(OcrError::backend(format!(
            "unsupported ONNX output shape: {shape:?}"
        )));
    }

    let classes = alphabet.len() + 1;
    let (sequence_len, layout) = match dims.as_slice() {
        [seq, class] if *class == classes => (*seq, OutputLayout::SequenceMajor),
        [class, seq] if *class == classes => (*seq, OutputLayout::ClassMajor),
        [] | [1] => (1, OutputLayout::SequenceMajor),
        other => {
            return Err(OcrError::backend(format!(
                "unexpected ONNX output dimensions {other:?} for alphabet of size {classes}",
                other = other
            )));
        }
    };

    if data.len() < sequence_len * classes {
        return Err(OcrError::backend(
            "onnx output buffer shorter than expected",
        ));
    }

    let mut result = String::new();
    let mut previous_idx: Option<usize> = None;
    let mut confidence_sum = 0.0f32;
    let mut confidence_count = 0usize;

    for step in 0..sequence_len {
        let mut max_logit = f32::NEG_INFINITY;
        for class in 0..classes {
            let value = get_logit(data, step, class, sequence_len, classes, layout);
            if value > max_logit {
                max_logit = value;
            }
        }
        let mut sum = 0.0f32;
        let mut best_index = 0usize;
        let mut best_prob = 0.0f32;
        for class in 0..classes {
            let value = get_logit(data, step, class, sequence_len, classes, layout);
            let exp = (value - max_logit).exp();
            sum += exp;
            if exp > best_prob {
                best_prob = exp;
                best_index = class;
            }
        }
        if sum <= 0.0 {
            continue;
        }
        let prob = best_prob / sum;
        if best_index != 0 && previous_idx != Some(best_index) {
            if let Some(character) = alphabet.get(best_index - 1) {
                result.push(*character);
                confidence_sum += prob;
                confidence_count += 1;
            }
        }
        if best_index == 0 {
            previous_idx = None;
        } else {
            previous_idx = Some(best_index);
        }
    }

    let confidence = if confidence_count > 0 {
        Some(confidence_sum / confidence_count as f32)
    } else {
        None
    };
    Ok((result, confidence))
}

#[derive(Clone, Copy)]
enum OutputLayout {
    SequenceMajor,
    ClassMajor,
}

fn get_logit(
    data: &[f32],
    step: usize,
    class: usize,
    sequence_len: usize,
    classes: usize,
    layout: OutputLayout,
) -> f32 {
    match layout {
        OutputLayout::SequenceMajor => data[step * classes + class],
        OutputLayout::ClassMajor => data[class * sequence_len + step],
    }
}

fn default_alphabet() -> Vec<char> {
    // Basic ASCII subset suitable for English subtitle content.
    "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ!\"#$%&'()*+,-./:;<=>?@[]^_`{|}~ "
        .chars()
        .collect()
}
