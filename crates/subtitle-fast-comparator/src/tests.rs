use subtitle_fast_types::{PlaneFrame, RoiConfig};

use crate::pipeline::PreprocessSettings;
use crate::{BitsetCoverComparator, SparseChamferComparator, SubtitleComparator};

fn frame_from_pixels(width: usize, height: usize, data: &[u8]) -> PlaneFrame {
    PlaneFrame::from_owned(width as u32, height as u32, width, None, data.to_vec()).unwrap()
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
fn sparse_chamfer_identical_frames_match() {
    let comparator = SparseChamferComparator::new(PreprocessSettings {
        target: 210,
        delta: 20,
    });
    let mut data = vec![30u8; 12 * 12];
    for y in 3..9 {
        for x in 2..10 {
            data[y * 12 + x] = 210;
        }
    }
    let frame = frame_from_pixels(12, 12, &data);
    let roi = full_roi();
    let features = comparator.extract(&frame, &roi).unwrap();
    let report = comparator.compare(&features, &features);
    assert!(report.same_segment);
    assert!(report.similarity >= 0.9);
}

#[test]
fn sparse_chamfer_detects_shift_and_style() {
    let comparator = SparseChamferComparator::new(PreprocessSettings {
        target: 220,
        delta: 25,
    });
    let mut base = vec![10u8; 20 * 12];
    for y in 4..8 {
        for x in 3..15 {
            base[y * 20 + x] = 230;
        }
    }
    let mut shifted = vec![10u8; 20 * 12];
    for y in 5..9 {
        for x in 5..17 {
            shifted[y * 20 + x] = 230;
        }
    }
    let mut thin = vec![10u8; 20 * 12];
    for y in 5..7 {
        for x in 3..15 {
            thin[y * 20 + x] = 230;
        }
    }
    let roi = full_roi();
    let frame_base = frame_from_pixels(20, 12, &base);
    let frame_shifted = frame_from_pixels(20, 12, &shifted);
    let frame_thin = frame_from_pixels(20, 12, &thin);
    let feat_base = comparator.extract(&frame_base, &roi).unwrap();
    let feat_shifted = comparator.extract(&frame_shifted, &roi).unwrap();
    let feat_thin = comparator.extract(&frame_thin, &roi).unwrap();
    let aligned = comparator.compare(&feat_base, &feat_shifted);
    assert!(aligned.same_segment);
    let style = comparator.compare(&feat_base, &feat_thin);
    assert!(!style.same_segment);
}

#[test]
fn bitset_cover_identical_frames_match() {
    let comparator = BitsetCoverComparator::new(PreprocessSettings {
        target: 200,
        delta: 15,
    });
    let mut pixels = vec![5u8; 16 * 12];
    for y in 4..8 {
        for x in 3..13 {
            pixels[y * 16 + x] = 205;
        }
    }
    let frame = frame_from_pixels(16, 12, &pixels);
    let roi = full_roi();
    let features = comparator.extract(&frame, &roi).unwrap();
    let report = comparator.compare(&features, &features);
    assert!(report.same_segment);
    assert!(report.similarity > 0.95);
}

#[test]
fn bitset_cover_detects_large_offset() {
    let comparator = BitsetCoverComparator::new(PreprocessSettings {
        target: 210,
        delta: 20,
    });
    let mut base = vec![0u8; 24 * 14];
    for y in 5..9 {
        for x in 4..18 {
            base[y * 24 + x] = 220;
        }
    }
    let mut shifted = vec![0u8; 24 * 14];
    for y in 7..11 {
        for x in 8..22 {
            shifted[y * 24 + x] = 220;
        }
    }
    let roi = full_roi();
    let frame_a = frame_from_pixels(24, 14, &base);
    let frame_b = frame_from_pixels(24, 14, &shifted);
    let feat_a = comparator.extract(&frame_a, &roi).unwrap();
    let feat_b = comparator.extract(&frame_b, &roi).unwrap();
    let report = comparator.compare(&feat_a, &feat_b);
    assert!(!report.same_segment);
    assert!(report.similarity < 0.9);
}
