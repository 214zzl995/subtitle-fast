# subtitle-fast-decoder

`subtitle-fast-decoder` provides interchangeable H.264 decoders that output NV12 frames by default. Pipelines request a
decoder, receive an async stream of frames, and can switch backends when the preferred option is unavailable.

## How decoding is orchestrated

1. **Build a configuration** – callers either use defaults or read from environment variables/CLI options to decide which
   backend to try, which input to open, and how many frames to buffer.
2. **Instantiate a backend** – the crate exposes factory helpers that negotiate with FFmpeg, VideoToolbox, D3D11/DXVA on
   Windows, Windows Media Foundation, or a lightweight mock backend compiled for CI.
3. **Stream frames** – once a backend is active it produces `VideoFrame` values containing NV12 planes (Y + UV) or, when
   explicitly requested on macOS VideoToolbox, a native CVPixelBuffer handle plus metadata. Frames are delivered through
   an async stream that respects backpressure.

If a backend fails to initialise (for example because the platform libraries are missing), callers can fall back to another
compiled backend before surfacing the error.

## Feature flags

| Feature | Description |
| ------- | ----------- |
| `backend-ffmpeg` | Uses `ffmpeg-next` to decode H.264 in a portable manner. |
| `backend-videotoolbox` | Enables hardware-accelerated decoding on macOS. |
| `backend-dxva` | Uses D3D11/DXVA video decoding on Windows for GPU-backed NV12 output. |
| `backend-mft` | Enables Windows Media Foundation decoding (Windows only). |

When no feature is enabled, only the lightweight mock backend is compiled. GitHub CI automatically enables the mock backend
so tests can exercise downstream logic without native dependencies.

### Static FFmpeg bundle (optional)
- Run `scripts/build-ffmpeg-min.sh` (Bash script) to download and build a trimmed FFmpeg (H.264 decoder + `mov`/`matroska`/`mpegts` demuxers, `buffer`/`buffersink`/`format`/`scale` filters) as static libraries under `target/ffmpeg-min`. Override with `FFMPEG_VERSION`, `PREFIX`, or `BUILD_DIR` as needed.
- Windows: build via MSYS2/MinGW (or similar) and run `./scripts/build-ffmpeg-min.sh`, then `cargo build --release --features backend-ffmpeg`.
- `.cargo/config.toml` sets `FFMPEG_DIR=target/ffmpeg-min` (and `PKG_CONFIG_PATH` to the matching pkg-config dir) so `ffmpeg-sys-next` links the trimmed bundle automatically when you build with the FFmpeg backend enabled.
- If you prefer your own FFmpeg build, point `FFMPEG_DIR` (and `PKG_CONFIG_PATH`) to it before building to bypass the script output.
- Prereqs: `curl`, `make`, and an assembler (`nasm` or `yasm`) available in `PATH`; `pkg-config` is helpful but not required when using `FFMPEG_DIR`.

## Configuration knobs

- Env vars: `SUBFAST_BACKEND`, `SUBFAST_INPUT`, and `SUBFAST_CHANNEL_CAPACITY` feed into `Configuration::from_env`.
- Output format: `Configuration::output_format` defaults to NV12; `OutputFormat::CVPixelBuffer` is only supported
  by the VideoToolbox backend and must be set in code (no env override).
- Default backend: the first compiled backend is chosen in priority order (mock on CI; VideoToolbox then FFmpeg on macOS;
  DXVA then MFT then FFmpeg on Windows; FFmpeg elsewhere).
- Channel capacity: `channel_capacity` limits the internal frame queue and governs backpressure.

## VideoToolbox CVPixelBuffer output (macOS)

When you need access to the native `CVPixelBuffer` handle from VideoToolbox, request handle output in code and wrap the
pointer into gpui's CoreVideo type yourself:

```rust
use subtitle_fast_decoder::{Backend, Configuration, OutputFormat};

let config = Configuration {
    backend: Backend::VideoToolbox,
    input: Some(input_path),
    output_format: OutputFormat::CVPixelBuffer,
    ..Configuration::default()
};

let mut stream = config.create_provider()?.into_stream();
while let Some(frame) = stream.next().await {
    let frame = frame?;
    let native = frame.native().expect("native handle output requested");
    let handle = native.handle();

    #[cfg(target_os = "macos")]
    unsafe {
        use core_foundation::base::TCFType;
        use core_video::pixel_buffer::{CVPixelBuffer, CVPixelBufferRef};

        let buffer = CVPixelBuffer::wrap_under_get_rule(handle as CVPixelBufferRef);
        // Example gpui usage:
        // window.paint_surface(bounds, buffer);
    }
}
```

`wrap_under_get_rule` retains the buffer, so the gpui `CVPixelBuffer` stays valid even after the `VideoFrame` is dropped.

## Error handling

All failures map to `FrameError` variants:

- `Unsupported` – the chosen backend was not compiled into this build.
- `BackendFailure` – the native backend returned an error string.
- `Configuration` – invalid environment variable or configuration input.
- `InvalidFrame` – safety checks on the decoded buffer failed (e.g., insufficient bytes for NV12 Y/UV planes).
- `Io` – filesystem-related issues while reading from disk-backed inputs.

Callers are expected to surface these errors to users and optionally try a different backend before aborting.
