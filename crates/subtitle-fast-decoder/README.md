# subtitle-fast-decoder

`subtitle-fast-decoder` provides interchangeable H.264 decoders that output monochrome Y-plane frames. Pipelines request a
decoder, receive an async stream of frames, and can switch backends when the preferred option is unavailable.

## How decoding is orchestrated

1. **Build a configuration** – callers either use defaults or read from environment variables/CLI options to decide which
   backend to try, which input to open, and how many frames to buffer.
2. **Instantiate a backend** – the crate exposes factory helpers that negotiate with FFmpeg, VideoToolbox, Windows Media
   Foundation, or a lightweight mock backend compiled for CI.
3. **Stream frames** – once a backend is active it produces `YPlaneFrame` values containing luma data, dimensions, stride,
   timestamps, and (when available) frame indices. Frames are delivered through an async stream that respects backpressure.

If a backend fails to initialise (for example because the platform libraries are missing), callers can fall back to another
compiled backend before surfacing the error.

## Feature flags

| Feature | Description |
| ------- | ----------- |
| `backend-ffmpeg` | Uses `ffmpeg-next` to decode H.264 in a portable manner. |
| `backend-videotoolbox` | Enables hardware-accelerated decoding on macOS. |
| `backend-mft` | Enables Windows Media Foundation decoding (Windows only). |

When no feature is enabled, only the lightweight mock backend is compiled. GitHub CI automatically enables the mock backend
so tests can exercise downstream logic without native dependencies.

## Configuration knobs

- Env vars: `SUBFAST_BACKEND`, `SUBFAST_INPUT`, and `SUBFAST_CHANNEL_CAPACITY` feed into `Configuration::from_env`.
- Default backend: the first compiled backend is chosen in priority order (mock on CI; VideoToolbox then FFmpeg on macOS;
  MFT then FFmpeg on Windows/other).
- Channel capacity: `channel_capacity` limits the internal frame queue and governs backpressure.

## Error handling

All failures map to `YPlaneError` variants:

- `Unsupported` – the chosen backend was not compiled into this build.
- `BackendFailure` – the native backend returned an error string.
- `Configuration` – invalid environment variable or configuration input.
- `InvalidFrame` – safety checks on the decoded buffer failed (e.g., insufficient bytes for the stride/height).
- `Io` – filesystem-related issues while reading from disk-backed inputs.

Callers are expected to surface these errors to users and optionally try a different backend before aborting.
