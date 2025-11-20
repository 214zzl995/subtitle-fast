# subtitle-fast-validator

`subtitle-fast-validator` contains the detection heuristics that decide whether a frame holds a subtitle band and where that
band is located. It powers the detection stage inside the CLI but can also be reused in other applications that need the
same luma-band analysis.

## Detection strategy

1. **Focus on the region of interest** – configuration describes where subtitles usually appear. Frames are cropped to that
   band before any heavy processing begins.
2. **Highlight subtitle-coloured pixels** – pixels whose brightness falls within the expected subtitle range are marked in a
   binary mask.
3. **Group neighbouring pixels** – runs of bright pixels are connected into blobs so individual characters merge into full
   lines even when there are small gaps.
4. **Filter and merge** – blobs are scored by size, aspect ratio, and fill density. Overlapping blobs are merged into up to
   four candidate lines per frame, each with a confidence score.
5. **Track through time** – consecutive frames are compared so that persistent lines become confirmed subtitle segments
   while short-lived noise is discarded.

The crate exposes a `FrameValidator` type that wraps this workflow and returns per-frame results including the best regions
and their scores. Pipelines can also supply a temporary ROI override when subtitles are known to appear elsewhere.

## Configuration and detectors

- Detector kinds: `auto` (default) tries projection-band then integral-band; `macos-vision` is available on macOS when the
  `detector-vision` feature is enabled.
- ROI: provide an `RoiConfig` to focus detection on a portion of the frame (values are normalised 0–1).
- Luma band tuning: `target` and `delta` (defaults 230/12) control which pixel intensities are treated as subtitle
  candidates.
- Debugging: set `REGION_DEBUG=1` to print per-region debug lines while running detectors.

## Feature flags

| Feature | Description |
| ------- | ----------- |
| `detector-vision` | Enables the Apple Vision-based detector (macOS only). |

When the feature is disabled the crate still provides the luma-band detector, which is cross-platform and requires no
native dependencies.
