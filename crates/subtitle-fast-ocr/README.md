# subtitle-fast-ocr

`subtitle-fast-ocr` defines the abstraction that turns luma-plane crops into recognised text. It supplies shared data
structures plus optional engines tailored for macOS.

## OCR flow at a glance

1. **Prepare the plane** – callers turn a `VideoFrame` into a compact `LumaPlane` buffer.
2. **Describe regions** – rectangular areas are collected as OCR regions, typically taken from the subtitle detector.
3. **Issue a request** – the `OcrEngine` trait receives the plane and regions, performs recognition, and returns text
   fragments with optional confidence values.

The trait also offers a warm-up hook so engines can preload models or allocate resources before the first recognition call.

## Engines

- `VisionOcrEngine` (macOS, behind `engine-vision`) uses Apple Vision.
- `NoopOcrEngine` returns empty results and is handy for pipeline or benchmarking tests.
- Additional engines can be integrated by implementing `OcrEngine` and wiring it into the caller's configuration.

## Feature flags

| Feature | Description |
| ------- | ----------- |
| `engine-vision` | Enables the Apple Vision OCR backend (macOS only). |

With neither feature enabled the crate only exposes `NoopOcrEngine`, which is useful for pipeline testing without OCR.
