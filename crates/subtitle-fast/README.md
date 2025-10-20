# subtitle-fast (CLI crate)

This crate exposes the `subtitle-fast` binary. It coordinates decoding, detection, OCR, and file writing while delegating
specialised work to the other crates in the workspace.

## How the CLI drives the pipeline

1. **Load settings** – the CLI collects options from command-line flags, `subtitle-fast.toml`, and platform-specific config
   directories. It resolves paths, chooses defaults, and reports any overrides.
2. **Pick a decoder** – using the merged settings, the CLI instantiates one of the available decoder backends. If a backend
   fails to initialise, the next compatible option is tried automatically.
3. **Prepare frames** – frames are sorted into presentation order and sampled at a fixed cadence. A short history window is
   retained so the detector can backtrack when subtitles begin or end.
4. **Detect subtitles** – the validator crate scores each sampled frame and tracks potential subtitle bands through time.
   Confirmed tracks are passed downstream together with metadata describing their bounds.
5. **Run OCR and emit files** – cropped regions are recognised by the configured OCR engine, then merged into `.srt`
   subtitles and optional JSON/image dumps.

Each step runs asynchronously, allowing the CLI to keep decoding even when OCR is comparatively slow.

## Working with configuration

- Command-line flags take priority over configuration files and environment variables.
- The CLI derives sensible defaults (for example seven detection samples per second) and stores them alongside the final
  plan so the UI can explain how each setting was chosen.
- OCR model locations can be local paths or URLs. Remote models are downloaded to a cache directory on first use.

## Debug outputs

When requested, the CLI can:

- Save sampled frames with detection overlays to a directory of your choice.
- Write JSON files describing every detection decision and the resulting subtitles.

These diagnostics are invaluable when tuning detection thresholds or validating OCR results on new languages.

## Feature flags and platforms

- Decoder backends are toggled through features on `subtitle-fast-decoder` (`backend-ffmpeg`, `backend-videotoolbox`,
  `backend-mft`, or the always-available mock backend).
- OCR support depends on the target: macOS builds can enable Apple Vision (`ocr-vision`) and MLX VLM (`ocr-mlx-vlm`).
- Debug helpers are available on all platforms and require no extra features.

## Running the binary

```bash
cargo run --release -- \
  --output subtitles.srt \
  --detection-samples-per-second 7 \
  --ocr-backend auto \
  path/to/video.mp4
```

The CLI prints the selected decoder, progress updates as subtitles are recognised, and the final output paths.
