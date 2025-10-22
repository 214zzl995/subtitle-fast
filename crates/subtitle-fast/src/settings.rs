use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use directories::{BaseDirs, ProjectDirs};
use serde::Deserialize;

use crate::cli::{CliArgs, CliSources, DumpFormat, OcrBackend};
use subtitle_fast_validator::subtitle_detection::{DEFAULT_LUMA_DELTA, DEFAULT_LUMA_TARGET};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct FileConfig {
    output: Option<String>,
    debug: Option<DebugDumpFileConfig>,
    detection: Option<DetectionFileConfig>,
    decoder: Option<DecoderFileConfig>,
    ocr: Option<OcrFileConfig>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
struct DecoderFileConfig {
    backend: Option<String>,
    channel_capacity: Option<usize>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
struct DebugDumpFileConfig {
    image: Option<ImageDumpFileConfig>,
    json: Option<JsonDumpFileConfig>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
struct DetectionFileConfig {
    samples_per_second: Option<u32>,
    luma_target: Option<u8>,
    luma_delta: Option<u8>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
struct OcrFileConfig {
    backend: Option<String>,
    languages: Option<Vec<String>>,
    auto_detect_language: Option<bool>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
struct ImageDumpFileConfig {
    enable: Option<bool>,
    dir: Option<String>,
    format: Option<String>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
struct JsonDumpFileConfig {
    enable: Option<bool>,
    dir: Option<String>,
    frames: Option<String>,
    segments: Option<String>,
    pretty: Option<bool>,
}

#[derive(Debug)]
pub struct EffectiveSettings {
    pub output: PathBuf,
    pub debug: DebugOutputSettings,
    pub detection: DetectionSettings,
    pub decoder: DecoderSettings,
    pub ocr: OcrSettings,
}

#[derive(Debug)]
pub struct ResolvedSettings {
    pub settings: EffectiveSettings,
}

#[derive(Debug, Clone)]
pub struct DetectionSettings {
    pub samples_per_second: u32,
    pub luma_target: u8,
    pub luma_delta: u8,
}

#[derive(Debug, Clone)]
pub struct OcrSettings {
    pub backend: OcrBackend,
    pub languages: Vec<String>,
    pub auto_detect_language: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DecoderSettings {
    pub backend: Option<String>,
    pub channel_capacity: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct DebugOutputSettings {
    pub image: Option<ImageDumpSettings>,
    pub json: Option<JsonDumpSettings>,
}

#[derive(Debug, Clone)]
pub struct ImageDumpSettings {
    pub dir: PathBuf,
    pub format: DumpFormat,
}

#[derive(Debug, Clone)]
pub struct JsonDumpSettings {
    pub dir: PathBuf,
    pub segments_filename: String,
    pub frames_filename: String,
    pub pretty: bool,
}

const DEFAULT_FRAMES_JSON: &str = "frames.json";
const DEFAULT_SEGMENTS_JSON: &str = "segments.json";
const DEFAULT_IMAGE_DUMP_DIR: &str = "dump/frames";
const DEFAULT_JSON_DUMP_DIR: &str = "dump";

#[derive(Debug)]
pub enum ConfigError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    InvalidValue {
        path: Option<PathBuf>,
        field: &'static str,
        value: String,
    },
    NotFound {
        path: PathBuf,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io { path, source } => {
                write!(
                    f,
                    "failed to read config file {}: {}",
                    path.display(),
                    source
                )
            }
            ConfigError::Parse { path, source } => {
                write!(
                    f,
                    "failed to parse config file {}: {}",
                    path.display(),
                    source
                )
            }
            ConfigError::InvalidValue { path, field, value } => {
                if let Some(path) = path {
                    write!(
                        f,
                        "invalid value '{}' for '{}' in {}",
                        value,
                        field,
                        path.display()
                    )
                } else {
                    write!(f, "invalid value '{}' for '{}'", value, field)
                }
            }
            ConfigError::NotFound { path } => {
                write!(f, "config file {} does not exist", path.display())
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io { source, .. } => Some(source),
            ConfigError::Parse { source, .. } => Some(source),
            ConfigError::InvalidValue { .. } => None,
            ConfigError::NotFound { .. } => None,
        }
    }
}

pub fn resolve_settings(
    cli: &CliArgs,
    sources: &CliSources,
) -> Result<ResolvedSettings, ConfigError> {
    let (file, config_path) = load_config(cli.config.as_deref())?;
    merge(cli, sources, file, config_path)
}

fn load_config(path_override: Option<&Path>) -> Result<(FileConfig, Option<PathBuf>), ConfigError> {
    if let Some(path) = path_override {
        let path = path.to_path_buf();
        if !path.exists() {
            return Err(ConfigError::NotFound { path });
        }
        let contents = fs::read_to_string(&path).map_err(|source| ConfigError::Io {
            path: path.clone(),
            source,
        })?;
        let config = toml::from_str(&contents).map_err(|source| ConfigError::Parse {
            path: path.clone(),
            source,
        })?;
        return Ok((config, Some(path)));
    }

    if let Some(project_path) = project_config_path() {
        if project_path.exists() {
            let contents = fs::read_to_string(&project_path).map_err(|source| ConfigError::Io {
                path: project_path.clone(),
                source,
            })?;
            let config = toml::from_str(&contents).map_err(|source| ConfigError::Parse {
                path: project_path.clone(),
                source,
            })?;
            return Ok((config, Some(project_path)));
        }
    }

    let Some(default_path) = default_config_path() else {
        return Ok((FileConfig::default(), None));
    };
    if !default_path.exists() {
        return Ok((FileConfig::default(), None));
    }
    let contents = fs::read_to_string(&default_path).map_err(|source| ConfigError::Io {
        path: default_path.clone(),
        source,
    })?;
    let config = toml::from_str(&contents).map_err(|source| ConfigError::Parse {
        path: default_path.clone(),
        source,
    })?;
    Ok((config, Some(default_path)))
}

fn merge(
    cli: &CliArgs,
    sources: &CliSources,
    file: FileConfig,
    config_path: Option<PathBuf>,
) -> Result<ResolvedSettings, ConfigError> {
    let config_dir = config_path
        .as_ref()
        .and_then(|path| path.parent().map(|dir| dir.to_path_buf()));

    let FileConfig {
        output: file_output,
        debug: file_debug,
        detection: file_detection,
        decoder: file_decoder,
        ocr: file_ocr,
    } = file;

    let debug_cfg = file_debug.unwrap_or_default();
    let detection_cfg = file_detection.unwrap_or_default();
    let decoder_cfg = file_decoder.unwrap_or_default();
    let ocr_cfg = file_ocr.unwrap_or_default();

    let file_debug_image = debug_cfg.image.clone();
    let file_debug_json = debug_cfg.json.clone();

    let mut dump_format = cli.dump_format;
    if !sources.dump_format_from_cli {
        if let Some(format_str) = file_debug_image
            .as_ref()
            .and_then(|cfg| normalize_string(cfg.format.clone()))
        {
            dump_format = parse_dump_format(&format_str, config_path.as_ref())?;
        }
    }

    let image_enabled_config = file_debug_image
        .as_ref()
        .and_then(|cfg| cfg.enable)
        .unwrap_or(false);
    let image_enabled = cli.dump_dir.is_some() || image_enabled_config;

    let image_dir_path = if image_enabled {
        if let Some(dir) = cli.dump_dir.clone() {
            Some(expand_pathbuf(dir))
        } else if let Some(path) = file_debug_image
            .as_ref()
            .and_then(|cfg| normalize_string(cfg.dir.clone()))
            .and_then(|dir| resolve_path_from_config(dir, config_dir.as_deref()))
        {
            Some(path)
        } else {
            Some(
                resolve_path_from_config(DEFAULT_IMAGE_DUMP_DIR.to_string(), config_dir.as_deref())
                    .unwrap_or_else(|| PathBuf::from(DEFAULT_IMAGE_DUMP_DIR)),
            )
        }
    } else {
        None
    };

    let image_dump = image_dir_path.clone().map(|dir| ImageDumpSettings {
        dir,
        format: dump_format,
    });

    let json_enabled = file_debug_json
        .as_ref()
        .and_then(|cfg| cfg.enable)
        .unwrap_or(false);

    let json_dump = if json_enabled {
        let resolved_dir = file_debug_json
            .as_ref()
            .and_then(|cfg| normalize_string(cfg.dir.clone()))
            .and_then(|value| resolve_path_from_config(value, config_dir.as_deref()))
            .unwrap_or_else(|| {
                resolve_path_from_config(DEFAULT_JSON_DUMP_DIR.to_string(), config_dir.as_deref())
                    .unwrap_or_else(|| PathBuf::from(DEFAULT_JSON_DUMP_DIR))
            });

        let frames_filename = file_debug_json
            .as_ref()
            .and_then(|cfg| normalize_string(cfg.frames.clone()))
            .unwrap_or_else(|| DEFAULT_FRAMES_JSON.to_string());
        let segments_filename = file_debug_json
            .as_ref()
            .and_then(|cfg| normalize_string(cfg.segments.clone()))
            .unwrap_or_else(|| DEFAULT_SEGMENTS_JSON.to_string());
        let pretty = file_debug_json
            .as_ref()
            .and_then(|cfg| cfg.pretty)
            .unwrap_or(true);

        Some(JsonDumpSettings {
            dir: resolved_dir,
            segments_filename,
            frames_filename,
            pretty,
        })
    } else {
        None
    };

    let debug_output = DebugOutputSettings {
        image: image_dump,
        json: json_dump,
    };

    let detection_samples_per_second = resolve_detection_sps(
        cli.detection_samples_per_second,
        detection_cfg.samples_per_second,
        !sources.detection_sps_from_cli,
        config_path.as_ref(),
    )?;

    let detection_luma_target = cli
        .detection_luma_target
        .or(detection_cfg.luma_target)
        .unwrap_or(DEFAULT_LUMA_TARGET);

    let detection_luma_delta = cli
        .detection_luma_delta
        .or(detection_cfg.luma_delta)
        .unwrap_or(DEFAULT_LUMA_DELTA);

    let ocr_backend = resolve_ocr_backend(
        cli.ocr_backend,
        ocr_cfg.backend.clone(),
        !sources.ocr_backend_from_cli,
        config_path.as_ref(),
    )?;

    let ocr_languages = resolve_ocr_languages(
        &cli.ocr_languages,
        ocr_cfg.languages.clone(),
        !sources.ocr_languages_from_cli,
    );

    let auto_detect_language = resolve_auto_detect_language(
        cli.ocr_auto_detect_language,
        ocr_cfg.auto_detect_language,
        !sources.ocr_auto_detect_language_from_cli,
    );

    let decoder_channel_capacity = resolve_decoder_capacity(
        cli.decoder_channel_capacity,
        decoder_cfg.channel_capacity,
        !sources.decoder_channel_capacity_from_cli,
        config_path.as_ref(),
    )?;

    let decoder_backend = normalize_string(cli.backend.clone())
        .or_else(|| normalize_string(decoder_cfg.backend.clone()));

    let output_path = resolve_output_path(cli.output.clone(), file_output, config_dir.as_deref())?;

    let decoder_settings = DecoderSettings {
        backend: decoder_backend,
        channel_capacity: decoder_channel_capacity,
    };

    let settings = EffectiveSettings {
        output: output_path,
        debug: debug_output,
        detection: DetectionSettings {
            samples_per_second: detection_samples_per_second,
            luma_target: detection_luma_target,
            luma_delta: detection_luma_delta,
        },
        decoder: decoder_settings,
        ocr: OcrSettings {
            backend: ocr_backend,
            languages: ocr_languages,
            auto_detect_language,
        },
    };

    Ok(ResolvedSettings { settings })
}

fn default_config_path() -> Option<PathBuf> {
    ProjectDirs::from("rs", "subtitle-fast", "subtitle-fast")
        .map(|dirs| dirs.config_dir().join("config.toml"))
}

fn project_config_path() -> Option<PathBuf> {
    env::current_dir().ok().map(|dir| dir.join("config.toml"))
}

fn normalize_string(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn expand_pathbuf(path: PathBuf) -> PathBuf {
    match path.to_str() {
        Some(s) => expand_home_path(s),
        None => path,
    }
}

fn resolve_path_from_config(value: String, base: Option<&Path>) -> Option<PathBuf> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let expanded = expand_home_path(trimmed);
    if expanded.is_absolute() || base.is_none() {
        Some(expanded)
    } else {
        Some(base.unwrap().join(expanded))
    }
}

fn expand_home_path(value: &str) -> PathBuf {
    if value == "~" {
        if let Some(base) = BaseDirs::new() {
            return base.home_dir().to_path_buf();
        }
    } else if let Some(stripped) = value.strip_prefix("~/") {
        if let Some(base) = BaseDirs::new() {
            return base.home_dir().join(stripped);
        }
    }
    PathBuf::from(value)
}

fn parse_dump_format(value: &str, path: Option<&PathBuf>) -> Result<DumpFormat, ConfigError> {
    DumpFormat::from_str(value, false).map_err(|_| ConfigError::InvalidValue {
        path: path.cloned(),
        field: "dump_format",
        value: value.to_string(),
    })
}

fn parse_ocr_backend(value: &str, path: Option<&PathBuf>) -> Result<OcrBackend, ConfigError> {
    OcrBackend::from_str(value, false).map_err(|_| ConfigError::InvalidValue {
        path: path.cloned(),
        field: "ocr_backend",
        value: value.to_string(),
    })
}

fn resolve_ocr_backend(
    cli_backend: OcrBackend,
    file_backend: Option<String>,
    use_file: bool,
    config_path: Option<&PathBuf>,
) -> Result<OcrBackend, ConfigError> {
    if use_file {
        if let Some(value) = normalize_string(file_backend) {
            return parse_ocr_backend(&value, config_path);
        }
    }
    Ok(cli_backend)
}

fn resolve_detection_sps(
    cli_value: u32,
    file_value: Option<u32>,
    use_file: bool,
    config_path: Option<&PathBuf>,
) -> Result<u32, ConfigError> {
    if use_file {
        if let Some(value) = file_value {
            if value < 1 {
                return Err(ConfigError::InvalidValue {
                    path: config_path.cloned(),
                    field: "detection_samples_per_second",
                    value: value.to_string(),
                });
            }
            return Ok(value);
        }
    }
    Ok(cli_value)
}

fn normalize_language_iter<I, S>(values: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut normalized = Vec::new();
    for value in values {
        let trimmed = value.as_ref().trim();
        if trimmed.is_empty() {
            continue;
        }
        let candidate = trimmed.to_string();
        if normalized
            .iter()
            .any(|existing: &String| existing.as_str().eq_ignore_ascii_case(&candidate))
        {
            continue;
        }
        normalized.push(candidate);
    }
    normalized
}

fn resolve_ocr_languages(
    cli_languages: &[String],
    file_languages: Option<Vec<String>>,
    use_file: bool,
) -> Vec<String> {
    if !cli_languages.is_empty() && !use_file {
        return normalize_language_iter(cli_languages.iter());
    }
    if use_file {
        if let Some(list) = file_languages {
            let normalized = normalize_language_iter(list);
            if !normalized.is_empty() {
                return normalized;
            }
        }
    }
    normalize_language_iter(cli_languages.iter())
}

fn resolve_auto_detect_language(
    cli_value: Option<bool>,
    file_value: Option<bool>,
    use_file: bool,
) -> bool {
    if let Some(value) = cli_value {
        return value;
    }
    if use_file {
        if let Some(value) = file_value {
            return value;
        }
    }
    file_value.unwrap_or(true)
}

fn resolve_decoder_capacity(
    cli_value: Option<usize>,
    file_value: Option<usize>,
    use_file: bool,
    config_path: Option<&PathBuf>,
) -> Result<Option<usize>, ConfigError> {
    let mut capacity = cli_value;
    if let Some(0) = capacity {
        return Err(ConfigError::InvalidValue {
            path: None,
            field: "decoder_channel_capacity",
            value: "0".into(),
        });
    }
    if use_file {
        if let Some(value) = file_value {
            if value == 0 {
                return Err(ConfigError::InvalidValue {
                    path: config_path.cloned(),
                    field: "decoder_channel_capacity",
                    value: value.to_string(),
                });
            }
            capacity = Some(value);
        }
    }
    Ok(capacity)
}

fn resolve_output_path(
    cli_value: PathBuf,
    file_value: Option<String>,
    config_dir: Option<&Path>,
) -> Result<PathBuf, ConfigError> {
    let _ = file_value;
    let _ = config_dir;
    let mut path = expand_pathbuf(cli_value);

    // If CLI path points to an existing directory, join default filename.
    if path.metadata().map(|meta| meta.is_dir()).unwrap_or(false) {
        path = path.join("subtitles.srt");
    } else if path.file_name().is_none() {
        path.push("subtitles.srt");
    }

    ensure_srt_extension(&mut path);
    Ok(path)
}

fn ensure_srt_extension(path: &mut PathBuf) {
    let is_srt = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("srt"))
        .unwrap_or(false);
    if !is_srt {
        path.set_extension("srt");
    }
}
