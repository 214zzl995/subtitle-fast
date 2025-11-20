# subtitle-fast-comparator

`subtitle-fast-comparator` extracts lightweight features from subtitle regions and compares successive frames to decide
whether a subtitle line is unchanged, has appeared, or has disappeared. It feeds the CLI segmenter so subtitles can be
opened and closed at the right timestamps.

## Comparators

- `bitset-cover` – binarises the ROI around the configured target/delta, dilates the mask for small shifts, and measures
  coverage overlap. Fast and forgiving; ideal default.
- `sparse-chamfer` – samples edge points, aligns them with a chamfer distance field, and scores how many points land near
  similar edges. Picks up thinner strokes but is slower.

## Using the crate

```rust
use subtitle_fast_comparator::{ComparatorFactory, ComparatorKind, ComparatorSettings};

let settings = ComparatorSettings {
    kind: ComparatorKind::BitsetCover,
    target: 230,
    delta: 12,
};
let comparator = ComparatorFactory::new(settings).build();

let reference = comparator.extract(&frame_a, &roi).unwrap();
let candidate = comparator.extract(&frame_b, &roi).unwrap();
let report = comparator.compare(&reference, &candidate);

if report.same_segment {
    // Keep extending the current subtitle interval.
}
```

`target` and `delta` mirror the validator's luma-band tuning and should match the detector settings. The same `RoiConfig`
used by the detector should be passed here so both stages look at the same region.
