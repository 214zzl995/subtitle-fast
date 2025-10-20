# subtitle-fast

A Rust workspace for streaming Y-plane data from decoded H.264 frames. The workspace currently contains:

- `subtitle-fast-decoder`: a library crate that exposes the `YPlaneStreamProvider` trait alongside several native decoding
  backends.
- `subtitle-fast`: a CLI crate that wires the decoder into a binary for manual backend testing.

## Architecture Overview

- `core`: Shared primitives, including the `YPlaneStreamProvider` trait, `YPlaneFrame` metadata, and error handling.
- `config`: Runtime configuration, enabling backend selection via environment variables or direct API usage.
- `backends`: Concrete backend modules. FFmpeg, VideoToolbox, and Windows MFT all feed Y-plane data into an async
  Tokio stream.

Backends are toggled through Cargo features on the library crate:

| Feature | Description |
| ------- | ----------- |
| `backend-ffmpeg` | Uses `ffmpeg-next` bindings for pure-Rust decoding without invoking the CLI binary. |
| `backend-videotoolbox` | Hardware-accelerated decoding on macOS using VideoToolbox via Objective-C bindings. |
| `backend-mft` | Windows Media Foundation H.264 decoding for native Windows builds. |

## Running the CLI

1. Enable the desired backend features on the workspace member. The CLI forwards feature flags to the decoder crate.
2. Provide the input asset path as the first positional argument. Backend selection can be overridden with
   `SUBFAST_BACKEND`, and file paths can be supplied via `SUBFAST_INPUT` when invoking the library directly.
3. Run the binary:

```bash
cargo run --release -- <video-path>
```

To run with a specific backend (for example, FFmpeg):

```bash
SUBFAST_BACKEND=ffmpeg \
cargo run --release --features backend-ffmpeg -- <video-path>
```

## Testing

Integration tests live under the decoder crate and require backend-specific assets. For example, to exercise the FFmpeg
backend:

```bash
SUBFAST_TEST_ASSET=/path/to/video.mp4 \
cargo test -p subtitle-fast-decoder --features backend-ffmpeg
```

Backends that rely on platform-specific APIs (e.g., VideoToolbox) will only run on compatible targets.

## Continuous Integration

See `.github/workflows/ci.yml` for a minimal GitHub Actions pipeline that exercises formatting and tests with a selected
feature set.
