use subtitle_fast_types::{RoiConfig, YPlaneFrame};

#[derive(Copy, Clone, Debug)]
pub struct PreprocessSettings {
    pub target: u8,
    pub delta: u8,
}

impl PreprocessSettings {
    pub fn target_f32(&self) -> f32 {
        self.target as f32 / 255.0
    }

    pub fn delta_f32(&self) -> f32 {
        self.delta.max(1) as f32 / 255.0
    }
}

#[derive(Clone, Debug)]
pub struct MaskedPatch {
    pub width: usize,
    pub height: usize,
    pub original: Vec<f32>,
    pub masked: Vec<f32>,
    pub mask: Vec<f32>,
}

impl MaskedPatch {
    pub fn len(&self) -> usize {
        self.width * self.height
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub fn extract_masked_patch(
    frame: &YPlaneFrame,
    roi: &RoiConfig,
    settings: PreprocessSettings,
) -> Option<MaskedPatch> {
    let bounds = roi_bounds(frame, roi)?;
    let (x0, y0, x1, y1) = bounds;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let width = x1 - x0;
    let height = y1 - y0;
    if width == 0 || height == 0 {
        return None;
    }
    let stride = frame.stride();
    let data = frame.data();

    let mut original = Vec::with_capacity(width * height);
    let mut masked = Vec::with_capacity(width * height);
    let mut mask = Vec::with_capacity(width * height);
    let lo = settings.target.saturating_sub(settings.delta.max(1)) as f32 / 255.0;
    let hi = settings.target.saturating_add(settings.delta.max(1)) as f32 / 255.0;
    for y in y0..y1 {
        let row = y * stride;
        for x in x0..x1 {
            let value = data[row + x] as f32 / 255.0;
            original.push(value);
            if value >= lo && value <= hi {
                masked.push(value);
                mask.push(1.0);
            } else {
                masked.push(0.0);
                mask.push(0.0);
            }
        }
    }

    Some(MaskedPatch {
        width,
        height,
        original,
        masked,
        mask,
    })
}

fn roi_bounds(frame: &YPlaneFrame, roi: &RoiConfig) -> Option<(usize, usize, usize, usize)> {
    let frame_w = frame.width() as usize;
    let frame_h = frame.height() as usize;
    if frame_w == 0 || frame_h == 0 {
        return None;
    }
    let mut x0 = (roi.x.clamp(0.0, 1.0) * frame_w as f32).floor() as isize;
    let mut y0 = (roi.y.clamp(0.0, 1.0) * frame_h as f32).floor() as isize;
    let mut x1 = ((roi.x + roi.width).clamp(0.0, 1.0) * frame_w as f32).ceil() as isize;
    let mut y1 = ((roi.y + roi.height).clamp(0.0, 1.0) * frame_h as f32).ceil() as isize;

    x0 = x0.clamp(0, frame_w as isize - 1);
    y0 = y0.clamp(0, frame_h as isize - 1);
    x1 = x1.clamp(x0 + 1, frame_w as isize);
    y1 = y1.clamp(y0 + 1, frame_h as isize);

    if x1 <= x0 || y1 <= y0 {
        return None;
    }

    Some((x0 as usize, y0 as usize, x1 as usize, y1 as usize))
}
