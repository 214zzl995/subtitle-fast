# Project TODO: Y-Plane Extraction Module

## Architectural Foundations
- [x] Define crate structure for modular video decoding backends (e.g., `core`, `backends::videotoolbox`, `backends::ffmpeg`, `backends::openh264`, `backends::gstreamer`).
- [x] Specify shared trait (e.g., `YPlaneStreamProvider`) exposing async Tokio stream of per-frame Y-plane buffers.
- [x] Choose common pixel buffer representation (aligned byte slices, width/height metadata, timestamps) and error handling strategy.
- [x] Establish feature flags to toggle individual backends for platform-specific builds.

## Videotoolbox Backend (macOS)
- [x] Investigate availability of Rust bindings (`videotoolbox` / `core-video` crates) compatible with latest Rust.
- [x] Implement hardware-accelerated decoder that produces Y-plane planes via VideoToolbox.
- [x] Design keyframe-based chunking to parallelize decoding jobs across threads.
- [x] Ensure decoded frames are pushed into Tokio `mpsc` channel for streaming consumption.
- [x] Provide graceful fallback or compile-time gating for non-macOS targets.

## FFmpeg Backend
- [x] Add dependency on `ffmpeg-next` (or similar) crate with latest compatible version.
- [x] Implement streaming decoder that extracts Y-plane data without shelling out to `ffmpeg` binary.
- [x] Integrate async Tokio pipeline by offloading decode loop to blocking task feeding an async stream.
- [x] Manage resource cleanup and codec context reuse.

## OpenH264 Backend
- [x] Introduce `openh264` crate (Rust bindings) and verify licensing requirements.
- [x] Implement multi-threaded decoding strategy to mitigate slower parsing speed (e.g., per-keyframe segments).
- [x] Convert decoded frames to Y-plane buffers and push through async stream.
- [ ] Benchmark/monitor performance impacts.

## GStreamer Backend
- [x] Depend on `gstreamer` Rust bindings with required feature flags for H.264.
- [x] Build pipeline using appsrc/appsink (or similar) to access Y-plane data in real time.
- [x] Wrap GStreamer bus/event handling inside async-compatible stream producer.

## Cross-Cutting Concerns
- [x] Define configuration layer selecting backend at runtime or compile time.
- [ ] Implement unit/integration tests using sample H.264 assets for each backend (where supported). *(FFmpeg, OpenH264, and VideoToolbox covered; GStreamer pending)*
- [ ] Provide mock/test double for Y-plane provider for environments lacking native support. *(Removed during workspace split; reconsider lightweight stub if needed.)*
- [x] Document usage, backend selection, and platform requirements in README.
- [x] Set up CI to build with minimal backend (e.g., OpenH264) and run lint/tests.

## Future Enhancements
- [ ] Explore zero-copy integration with downstream consumers.
- [ ] Add metrics/logging for decode performance and errors.
- [ ] Investigate support for additional codecs or color formats.
