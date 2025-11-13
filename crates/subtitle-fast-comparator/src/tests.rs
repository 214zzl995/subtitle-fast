use subtitle_fast_decoder::YPlaneFrame;
use subtitle_fast_validator::subtitle_detection::RoiConfig;

use crate::preprocess::PreprocessSettings;
use crate::{
    ChamferEdgeComparator, HybridMaskComparator, SpectralHashComparator, StructuralDssimComparator,
    SubtitleComparator,
};

fn frame_from_pixels(width: usize, height: usize, data: &[u8]) -> YPlaneFrame {
    YPlaneFrame::from_owned(width as u32, height as u32, width, None, data.to_vec()).unwrap()
}

fn full_roi() -> RoiConfig {
    RoiConfig {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    }
}

#[test]
fn spectral_hash_identical_frames_match() {
    let comparator = SpectralHashComparator::new(PreprocessSettings {
        target: 210,
        delta: 20,
    });
    let frame = frame_from_pixels(8, 8, &[200; 64]);
    let roi = full_roi();
    let features = comparator.extract(&frame, &roi).unwrap();
    let report = comparator.compare(&features, &features);
    assert!(report.same_segment);
    assert!(report.similarity >= 0.95);
}

#[test]
fn spectral_hash_detects_changes() {
    let comparator = SpectralHashComparator::new(PreprocessSettings {
        target: 210,
        delta: 20,
    });
    let frame_a = frame_from_pixels(8, 8, &[200; 64]);
    let mut data_b = vec![200u8; 64];
    for value in data_b.iter_mut().take(16) {
        *value = 50;
    }
    let frame_b = frame_from_pixels(8, 8, &data_b);
    let roi = full_roi();
    let feat_a = comparator.extract(&frame_a, &roi).unwrap();
    let feat_b = comparator.extract(&frame_b, &roi).unwrap();
    let report = comparator.compare(&feat_a, &feat_b);
    assert!(!report.same_segment);
}

#[test]
fn structural_dssim_tracks_similarity() {
    let comparator = StructuralDssimComparator::new(PreprocessSettings {
        target: 180,
        delta: 30,
    });
    let frame_a = frame_from_pixels(10, 6, &[190; 60]);
    let frame_b = frame_from_pixels(10, 6, &[185; 60]);
    let roi = full_roi();
    let feat_a = comparator.extract(&frame_a, &roi).unwrap();
    let feat_b = comparator.extract(&frame_b, &roi).unwrap();
    let report = comparator.compare(&feat_a, &feat_b);
    assert!(report.same_segment);
}

#[test]
fn hybrid_mask_handles_shift() {
    let comparator = HybridMaskComparator::new(PreprocessSettings {
        target: 200,
        delta: 25,
    });
    let mut base = vec![20u8; 12 * 8];
    for y in 2..6 {
        for x in 1..7 {
            base[y * 12 + x] = 210;
        }
    }
    let mut shifted = vec![20u8; 12 * 8];
    for y in 2..6 {
        for x in 3..9 {
            shifted[y * 12 + x] = 215;
        }
    }
    let frame_a = frame_from_pixels(12, 8, &base);
    let frame_b = frame_from_pixels(12, 8, &shifted);
    let roi = full_roi();
    let feat_a = comparator.extract(&frame_a, &roi).unwrap();
    let feat_b = comparator.extract(&frame_b, &roi).unwrap();
    let report = comparator.compare(&feat_a, &feat_b);
    assert!(report.same_segment);
    assert!(report.similarity >= 0.78);
}

#[test]
fn chamfer_edge_penalizes_different_strokes() {
    let comparator = ChamferEdgeComparator::new(PreprocessSettings {
        target: 220,
        delta: 20,
    });
    let mut thick = vec![10u8; 16 * 12];
    for y in 4..8 {
        for x in 2..14 {
            thick[y * 16 + x] = 230;
        }
    }
    let mut thin = vec![10u8; 16 * 12];
    for y in 5..7 {
        for x in 2..14 {
            thin[y * 16 + x] = 230;
        }
    }
    let frame_thick = frame_from_pixels(16, 12, &thick);
    let frame_thin = frame_from_pixels(16, 12, &thin);
    let roi = full_roi();
    let feat_thick = comparator.extract(&frame_thick, &roi).unwrap();
    let feat_thin = comparator.extract(&frame_thin, &roi).unwrap();
    let report = comparator.compare(&feat_thick, &feat_thin);
    assert!(!report.same_segment);
}
