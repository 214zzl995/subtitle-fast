use std::time::Instant;

use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use subtitle_fast_decoder::{Backend, Configuration};
use subtitle_fast_types::DecoderError;

use crate::stage;

const COL_AVG: &str = "\x1b[33m"; // yellow-ish for averages
const COL_COUNT: &str = "\x1b[36m"; // cyan-ish for counts
const COL_RESET: &str = "\x1b[0m";

#[derive(Clone)]
pub struct ExecutionPlan {
    pub config: Configuration,
    pub backend_locked: bool,
    pub pipeline: stage::PipelineConfig,
}

pub async fn run(plan: ExecutionPlan) -> Result<(), DecoderError> {
    let ExecutionPlan {
        config,
        backend_locked,
        pipeline,
    } = plan;

    let available = Configuration::available_backends();
    if available.is_empty() {
        return Err(DecoderError::configuration(
            "no decoding backend available; rebuild with a backend feature such as \"backend-ffmpeg\"",
        ));
    }
    if !available.contains(&config.backend) {
        return Err(DecoderError::unsupported(config.backend.as_str()));
    }

    let mut attempt_config = config.clone();
    let mut tried = Vec::new();

    loop {
        if !tried.contains(&attempt_config.backend) {
            tried.push(attempt_config.backend);
        }

        let provider_started = Instant::now();
        let provider_result = attempt_config.create_provider();
        let provider_elapsed = provider_started.elapsed();

        let provider = match provider_result {
            Ok(provider) => {
                eprintln!(
                    "initialized decoder backend '{}' in {:.2?}",
                    attempt_config.backend.as_str(),
                    provider_elapsed
                );
                provider
            }
            Err(err) => {
                eprintln!(
                    "decoder backend '{}' failed to initialize in {:.2?}: {err}",
                    attempt_config.backend.as_str(),
                    provider_elapsed
                );
                if !backend_locked
                    && let Some(next_backend) = select_next_backend(&available, &tried)
                {
                    let failed_backend = attempt_config.backend;
                    eprintln!(
                        "backend {failed} failed to initialize ({reason}); trying {next}",
                        failed = failed_backend.as_str(),
                        reason = err,
                        next = next_backend.as_str()
                    );
                    attempt_config.backend = next_backend;
                    continue;
                }
                return Err(err);
            }
        };

        let pipeline_result = stage::build_pipeline(provider, &pipeline);

        let outcome = match pipeline_result {
            Ok(pipeline_streams) => drive_pipeline(pipeline_streams, &pipeline.output.path).await,
            Err(err) => Err((err, 0)),
        };

        match outcome {
            Ok(()) => return Ok(()),
            Err((err, seen)) => {
                if seen == 0
                    && !backend_locked
                    && let Some(next_backend) = select_next_backend(&available, &tried)
                {
                    let failed_backend = attempt_config.backend;
                    eprintln!(
                        "backend {failed} failed to decode ({reason}); trying {next}",
                        failed = failed_backend.as_str(),
                        reason = err,
                        next = next_backend.as_str()
                    );
                    attempt_config.backend = next_backend;
                    continue;
                }
                return Err(err);
            }
        }
    }
}

pub fn display_available_backends() {
    let names: Vec<&'static str> = Configuration::available_backends()
        .iter()
        .map(Backend::as_str)
        .collect();
    if names.is_empty() {
        println!("available backends: (none compiled)");
    } else {
        println!("available backends: {}", names.join(", "));
    }
}

pub fn parse_backend(value: &str) -> Result<Backend, DecoderError> {
    use std::str::FromStr;
    Backend::from_str(value)
}

fn select_next_backend(available: &[Backend], tried: &[Backend]) -> Option<Backend> {
    available
        .iter()
        .copied()
        .find(|backend| !tried.contains(backend))
}

async fn drive_pipeline(
    pipeline: stage::PipelineOutputs,
    output_path: &std::path::Path,
) -> Result<(), (DecoderError, u64)> {
    let mut processed = 0;
    let mut subtitles: Vec<stage::MergedSubtitle> = Vec::new();
    let mut stream = pipeline.stream;
    let mut progress = PipelineProgressBar::new("detect", pipeline.total_frames);

    while let Some(event) = stream.next().await {
        match event {
            Ok(update) => {
                processed = processed.max(update.progress.samples_seen);
                progress.update(&update.progress);
                apply_updates(&mut subtitles, &update.updates);
            }
            Err(err) => {
                let mapped = stage::pipeline_error_to_frame(err);
                progress.fail(&mapped.to_string());
                return Err((mapped, processed));
            }
        }
    }

    progress.finish(processed);
    sort_and_write(output_path, &subtitles)
        .await
        .map_err(|err| (err, processed))
}

struct PipelineProgressBar {
    bar: ProgressBar,
    total_frames: Option<u64>,
    finished: bool,
}

impl PipelineProgressBar {
    fn new(label: &'static str, total_frames: Option<u64>) -> Self {
        let bar = match total_frames {
            Some(total) => {
                let bar = ProgressBar::new(total);
                bar.set_style(bar_style());
                bar
            }
            None => {
                let bar = ProgressBar::new_spinner();
                bar.set_style(spinner_style());
                bar
            }
        };
        bar.set_prefix(label);

        Self {
            bar,
            total_frames,
            finished: false,
        }
    }

    fn update(&mut self, progress: &stage::PipelineProgress) {
        if let Some(total) = self.total_frames {
            let next = std::cmp::min(progress.latest_frame_index.saturating_add(1), total);
            self.bar.set_position(next);
        } else {
            self.bar.inc(1);
        }

        let det = format_ms(progress.det_ms);
        let seg = format_ms(progress.seg_ms);
        let ocr = format_ms(progress.ocr_ms);
        let avg_line = format!(
            "[{COL_AVG}   avg{COL_RESET}] fps {fps:>5.1} • det {det} • seg {seg} • ocr {ocr}",
            fps = progress.fps
        );
        let counts_line = format!(
            "[{COL_COUNT}counts{COL_RESET}] cues {cues} • merged {merged} • ocr-empty {empty}",
            cues = progress.cues,
            merged = progress.merged,
            empty = progress.ocr_empty
        );
        self.bar.set_message(format!("{avg_line}\n{counts_line}"));
    }

    fn fail(&mut self, reason: &str) {
        if self.finished {
            return;
        }
        self.finished = true;
        if let Some(total) = self.total_frames {
            let pos = std::cmp::min(self.bar.position(), total);
            self.bar.set_position(pos);
        }
        self.bar.abandon_with_message(format!(
            "failed after {} frames: {reason}",
            self.bar.position()
        ));
    }

    fn finish(&mut self, processed: u64) {
        if self.finished {
            return;
        }
        self.finished = true;
        if let Some(total) = self.total_frames {
            self.bar.set_position(total);
            self.bar
                .finish_with_message(format!("processed {total}/{total} frames"));
        } else {
            self.bar
                .finish_with_message(format!("processed {processed} frames"));
        }
    }
}

fn format_ms(value: f64) -> String {
    if value <= 0.0 {
        return "-- ms".to_string();
    }
    format!("{value:.1} ms")
}

fn bar_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{prefix:.bold} {bar:40.cyan/blue} {percent:>3.bold}% {pos:>5}/{len:<5} [{elapsed_precise:.dim}<{eta_precise:.dim}]\n{msg}",
    )
    .expect("invalid sampling bar template")
    .progress_chars("█▉▊▋▌▍▎▏ ")
}

fn spinner_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{prefix:.bold} {spinner:.cyan.bold} [{elapsed_precise:.dim}] {pos:>5}f\n{msg}",
    )
    .expect("invalid sampling spinner template")
    .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
}

fn apply_updates(subtitles: &mut Vec<stage::MergedSubtitle>, updates: &[stage::SubtitleUpdate]) {
    for update in updates {
        match update.kind {
            stage::SubtitleUpdateKind::New => {
                subtitles.push(update.subtitle.clone());
            }
            stage::SubtitleUpdateKind::Updated => {
                if let Some(existing) = subtitles
                    .iter_mut()
                    .find(|subtitle| subtitle.id == update.subtitle.id)
                {
                    *existing = update.subtitle.clone();
                } else {
                    subtitles.push(update.subtitle.clone());
                }
            }
        }
    }
}

async fn sort_and_write(
    output_path: &std::path::Path,
    subtitles: &[stage::MergedSubtitle],
) -> Result<(), DecoderError> {
    let mut ordered = subtitles.to_vec();
    stage::sort_subtitles(&mut ordered);
    let contents = stage::render_srt(&ordered);

    if let Some(parent) = output_path.parent().filter(|p| !p.as_os_str().is_empty())
        && let Err(err) = tokio::fs::create_dir_all(parent).await
    {
        return Err(DecoderError::configuration(format!(
            "failed to prepare subtitle directory {}: {err}",
            parent.display()
        )));
    }

    tokio::fs::write(output_path, contents)
        .await
        .map_err(|err| {
            DecoderError::configuration(format!(
                "failed to write subtitle file {}: {err}",
                output_path.display()
            ))
        })
}
