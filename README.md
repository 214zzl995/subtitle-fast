# subtitle-fast

An experimental Rust workspace for streaming Y-plane data from decoded H.264 frames. The crate exposes a trait-based
interface so multiple decoding backends can coexist behind a unified asynchronous stream.

## Architecture Overview

- `core`: Shared primitives, including the `YPlaneStreamProvider` trait, `YPlaneFrame` metadata, and error handling.
- `config`: Runtime configuration, enabling backend selection via environment variables or direct API usage.
- `backends`: Concrete backend modules. The FFmpeg implementation decodes frames in a blocking task and forwards Y-plane
  data through a Tokio `mpsc` channel. Mock and placeholder providers are included for environments lacking native
  libraries.

Backends are toggled through Cargo features:

| Feature | Description |
| ------- | ----------- |
| `backend-mock` (default) | Generates synthetic frames for testing and local development. |
| `backend-ffmpeg` | Uses `ffmpeg-next` bindings for pure-Rust decoding without invoking the CLI binary. |
| `backend-videotoolbox` | Placeholder for a future macOS VideoToolbox backend. |
| `backend-openh264` | Placeholder for an OpenH264-based decoder with multi-threaded support. |
| `backend-gstreamer` | Placeholder for a GStreamer pipeline using appsink. |

## Running

1. Ensure the desired backend feature is enabled. By default the mock backend runs, which does not require external
   dependencies.
2. Optionally configure runtime selection using environment variables:
   - `SUBFAST_BACKEND` &mdash; choose a backend (`mock`, `ffmpeg`, `videotoolbox`, `openh264`, `gstreamer`).
   - `SUBFAST_INPUT` &mdash; path to an H.264/MP4 asset when using FFmpeg or other file-based backends.
3. Execute the binary:

```bash
cargo run --release
```

To enable the FFmpeg backend:

```bash
SUBFAST_BACKEND=ffmpeg \
SUBFAST_INPUT=/path/to/video.mp4 \
cargo run --release --no-default-features --features backend-ffmpeg
```

## Testing

Run the standard tests with the default (mock) backend:

```bash
cargo test
```

To run FFmpeg integration tests, provide a valid test asset:

```bash
SUBFAST_TEST_ASSET=/path/to/video.mp4 cargo test --no-default-features --features backend-ffmpeg
```

## Continuous Integration

See `.github/workflows/ci.yml` for a minimal GitHub Actions pipeline that exercises formatting and tests with the default
feature set.
