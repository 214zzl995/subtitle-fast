# subtitle-fast

subtitle-fast is a Rust workspace that turns H.264 video files into subtitle tracks through an async pipeline of decoders, detectors, and OCR backends. Crate-level docs cover the deeper heuristics: [`subtitle-fast`](crates/subtitle-fast/README.md), [`subtitle-fast-decoder`](crates/subtitle-fast-decoder/README.md), [`subtitle-fast-validator`](crates/subtitle-fast-validator/README.md), [`subtitle-fast-comparator`](crates/subtitle-fast-comparator/README.md), and [`subtitle-fast-ocr`](crates/subtitle-fast-ocr/README.md).

- [中文文档](README-zh.md)

License: [MIT](LICENSE)

## Quick start

- Prerequisites: Rust (stable), and the native pieces you plan to use (FFmpeg libs for `backend-ffmpeg`, built-in VideoToolbox on macOS, D3D11/DXVA-capable drivers on Windows, Media Foundation for the fallback MFT backend, and Apple Vision frameworks for `ocr-vision`).
- Minimal run (uses the compiled decoder backends and Vision OCR on macOS):

```bash
cargo run --release -- --output subtitles.srt path/to/video.mp4
```

- On platforms without macOS SDKs, disable the mac-only defaults:

```bash
cargo run --release --no-default-features \
  -- --backend ffmpeg --output subtitles.srt path/to/video.mp4
```

## Backends and features

**Decoders** (enabled by default in `subtitle-fast-decoder`)
- `backend-ffmpeg` (FFmpeg; portable).
- `backend-videotoolbox` (macOS hardware decode).
- `backend-dxva` (Windows D3D11/DXVA hardware decode).
- `backend-mft` (Windows Media Foundation).
- `mock` is always available and useful for CI or dry runs (`--backend mock`).

The CLI picks the first compiled backend in priority order (mock on CI; VideoToolbox then FFmpeg on macOS; DXVA then MFT then FFmpeg on Windows; FFmpeg elsewhere) and falls back if a backend fails, preserving backpressure when downstream stages slow down.

**OCR**
- `ocr-vision` enables Apple Vision on macOS (`--ocr-backend vision` or `auto` when available).
- Without Vision, the noop OCR engine keeps the pipeline running for benchmarking (`--ocr-backend noop`).

**Detection helpers**
- `detector-vision` (macOS) is available on the validator crate; disable mac-only flags on other targets.

## Configuration

Configuration precedence: CLI flags > `--config <path>` > `./config.toml` > platform config dir (e.g. `~/.config/subtitle-fast/config.toml`). Copy `config.toml.example` as a starting point.

```toml
[detection]
samples_per_second = 7
target = 230
delta = 12
# comparator = "bitset-cover"
# roi = { x = 0.0, y = 0.75, width = 1.0, height = 0.25 } # normalized 0-1; omit/zero → full frame

[decoder]
# backend = "dxva"
# channel_capacity = 32
```

CLI flags like `--detector-target`, `--detector-delta`, `--roi x,y,width,height`, `--backend`, and `--ocr-backend` override the file settings. Omit the ROI flag or use a zero-sized ROI to scan the full frame.

## Pipeline overview

1. Select a decoder and stream NV12 frames ([decoder](crates/subtitle-fast-decoder/README.md)).
2. Sample frames and score subtitle bands ([validator](crates/subtitle-fast-validator/README.md)).
3. Compare regions across frames to spot line starts/ends ([comparator](crates/subtitle-fast-comparator/README.md)).
4. Run OCR on confirmed regions ([ocr](crates/subtitle-fast-ocr/README.md)) and emit `.srt` cues.

Each stage consumes an async stream and preserves backpressure so decoding slows naturally when OCR becomes the bottleneck.

## Debugging and testing

- Smoke test the pipeline with a short clip: `cargo run --release -- --backend mock --output subtitles.srt path/to/video.mp4`.
- Decoder integration tests require a sample clip and the matching feature, e.g.:

```bash
SUBFAST_TEST_ASSET=/path/to/video.mp4 \
cargo test -p subtitle-fast-decoder --features backend-ffmpeg
```

## Performance snapshot

- A 2h01m 1080p H.264 (High, yuv420p, 29.97 fps, ~5.0 Mbps video with AAC 48 kHz stereo ~255 kb/s; overall ~5.26 Mbps) sample completes in roughly 1m40s on a Mac mini M4 using `cargo run --release` with default features (VideoToolbox + Vision available), issuing about 3,622 OCR requests over the run.
