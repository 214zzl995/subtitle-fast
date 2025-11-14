# subtitle-fast

subtitle-fast is a Rust workspace for turning video files into subtitle tracks. The workspace is made of four crates that
are combined by the CLI binary:

- [`subtitle-fast`](crates/subtitle-fast/README.md) – drives the async pipeline and owns CLI configuration.
- [`subtitle-fast-decoder`](crates/subtitle-fast-decoder/README.md) – selects a decoding backend and exposes frames as a
  luma (Y) plane stream.
- [`subtitle-fast-validator`](crates/subtitle-fast-validator/README.md) – judges which frames contain subtitle bands and
  reports candidate regions.
- [`subtitle-fast-ocr`](crates/subtitle-fast-ocr/README.md) – converts cropped subtitle regions into text.

Together these crates let the CLI stream frames, detect subtitle bands, push them through OCR, and emit `.srt` files plus
optional debug material.

## End-to-end data flow

Running the CLI walks through a fixed set of stages:

1. **Decoder selection** – the CLI constructs a decoder configuration from CLI flags, config files, and environment
   variables, then instantiates the first available backend (FFmpeg, VideoToolbox, Windows MFT, or a mock fallback). The
   decoder yields raw Y-plane frames with timestamps.
2. **Frame preparation** – frames are sorted into presentation order and sampled at a configurable rate so later stages
   only inspect the minimum number of frames required for stable detection.
3. **Subtitle detection** – sampled frames flow into the validator crate, which scores each frame and maintains tracks of
   regions that look like subtitle bands.
4. **OCR + authoring** – confirmed tracks are expanded into OCR regions, recognised by the configured OCR backend, and
   emitted as `.srt` cues (with optional JSON metadata and annotated images for debugging).

Each stage consumes an async stream of frames or regions and forwards a new stream downstream. Backpressure is preserved,
so the decoder naturally slows down when OCR becomes the bottleneck.

## Detection approach and limitations

The validator crate focuses on a luma-band heuristic: it inspects a configurable strip near the bottom of each frame,
looks for clusters of pixels whose brightness matches subtitle text, merges neighbouring components into lines, and keeps
the most stable regions. The CLI then tracks those regions across consecutive frames to determine when subtitles appear or
disappear.

Although this luma-band backend can deliver extremely high throughput, it remains experimental. Subtitle styles differ
widely across sources, and the same parameters that excel on one show may fail to stabilise thin fonts, stylised karaoke,
or unconventional layouts. Expect to revisit detector tuning for new content, and pair it with OCR or alternative
detectors when consistency matters more than raw speed.

This strategy works well for broadcast-style subtitles but there is one notable gap today: if the video already contains a
graphic that strongly resembles a subtitle and a genuine subtitle band later appears within that same area, the detector
may treat the entire span as part of the static graphic. In that case the real subtitle band can be lost. Improving this
behaviour would require a stronger separation between persistent overlays and newly arrived lines.

## Configuration and models

Configuration is merged from CLI flags, a `subtitle-fast.toml` file (if present or specified via `--config`), and
platform-specific config directories. Paths are normalised, defaults are applied, and the resulting plan records where
frames should be dumped, which OCR backend to use, and how aggressive detection should be.

Detector tuning intentionally sticks to the two historical knobs: `--detector-target` and `--detector-delta` (or the
`target` / `delta` keys under `[detection]` in `subtitle-fast.toml`). Every other heuristic now relies on the detector's
internal defaults so behaviour stays predictable across runs.

OCR model paths can be provided as local paths or HTTP(S)/`file://` URLs. Remote models are cached locally the first time
they are used so subsequent runs start quickly.

## Debugging aids

When debug outputs are enabled (either via CLI or config), the pipeline can:

- Persist sampled frames with detection overlays to disk for later visual inspection.
- Dump per-frame and per-segment JSON files containing bounding boxes, intermediate scores, and the indices considered
  when constructing each subtitle.

These dumps are invaluable when tuning detector parameters or verifying OCR output.

## Building and running

```bash
# Build with release optimisations and run the CLI. Provide an input path as the final argument.
cargo run --release -- --output subtitles.srt path/to/video.mp4
```

Key CLI flags include:

- `--backend` – lock decoding to a specific backend (`mock`, `ffmpeg`, `videotoolbox`, `mft`).
- `--detection-samples-per-second` – tune the temporal sampling budget.
- `--ocr-backend` plus `--ocr-language` flags – steer OCR behaviour.
- `--dump-dir` and `--dump-format` – emit annotated frames for visual inspection.

Platform-specific decoding backends and OCR engines are controlled through Cargo features exposed by each crate. The
workspace enables a "mock" backend automatically for CI environments and can be compiled with FFmpeg, VideoToolbox, or
Windows MFT support when those features are toggled.

## Testing

The decoder crate hosts integration tests that rely on backend-specific assets:

```bash
# Example: run FFmpeg-backed tests once SUBFAST_TEST_ASSET is set to a valid H.264 clip
SUBFAST_TEST_ASSET=/path/to/video.mp4 \
cargo test -p subtitle-fast-decoder --features backend-ffmpeg
```

The CLI pipeline itself can be smoke-tested against short samples by invoking `cargo run --release` with the desired
feature flags and CLI options.
