use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use directories::{BaseDirs, ProjectDirs};
use serde::Deserialize;

use crate::cli::{CliArgs, CliSources, DetectionBackend, DumpFormat};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct FileConfig {
    backend: Option<String>,
    dump_dir: Option<String>,
    dump_format: Option<String>,
    dump: Option<DumpFileConfig>,
    detection_samples_per_second: Option<u32>,
    detection_backend: Option<String>,
    onnx_model: Option<String>,
    detection_luma_target: Option<u8>,
    detection_luma_delta: Option<u8>,
    decoder_channel_capacity: Option<usize>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
struct DumpFileConfig {
    image: Option<ImageDumpFileConfig>,
    json: Option<JsonDumpFileConfig>,
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
    pub backend: Option<String>,
    pub image_dump: Option<ImageDumpSettings>,
    pub json_dump: Option<JsonDumpSettings>,
    pub detection_samples_per_second: u32,
    pub detection_backend: DetectionBackend,
    pub onnx_model: Option<String>,
    pub onnx_model_from_cli: bool,
    pub config_dir: Option<PathBuf>,
    pub detection_luma_target: Option<u8>,
    pub detection_luma_delta: Option<u8>,
    pub decoder_channel_capacity: Option<usize>,
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
) -> Result<EffectiveSettings, ConfigError> {
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
) -> Result<EffectiveSettings, ConfigError> {
    let config_dir = config_path
        .as_ref()
        .and_then(|path| path.parent().map(|dir| dir.to_path_buf()));

    let FileConfig {
        backend: file_backend,
        dump_dir: file_dump_dir,
        dump_format: file_dump_format,
        dump: file_dump_sections,
        detection_samples_per_second: file_detection_sps,
        detection_backend: file_detection_backend,
        onnx_model: file_onnx_model,
        detection_luma_target: file_luma_target,
        detection_luma_delta: file_luma_delta,
        decoder_channel_capacity: file_decoder_channel_capacity,
    } = file;

    let (file_dump_image, file_dump_json) = match file_dump_sections {
        Some(section) => (section.image, section.json),
        None => (None, None),
    };

    let mut backend = normalize_string(cli.backend.clone());
    if backend.is_none() {
        backend = normalize_string(file_backend);
    }

    let legacy_dump_dir = normalize_string(file_dump_dir.clone());

    let mut dump_format = cli.dump_format;
    if !sources.dump_format_from_cli {
        if let Some(format_str) = file_dump_image
            .as_ref()
            .and_then(|cfg| normalize_string(cfg.format.clone()))
            .or_else(|| normalize_string(file_dump_format))
        {
            dump_format = parse_dump_format(&format_str, config_path.as_ref())?;
        }
    }

    let image_enabled_config = match file_dump_image.as_ref().and_then(|cfg| cfg.enable) {
        Some(value) => value,
        None => legacy_dump_dir.is_some(),
    };
    let image_enabled = cli.dump_dir.is_some() || image_enabled_config;

    let image_dir_path = if image_enabled {
        if let Some(dir) = cli.dump_dir.clone() {
            Some(expand_pathbuf(dir))
        } else if let Some(path) = file_dump_image
            .as_ref()
            .and_then(|cfg| normalize_string(cfg.dir.clone()))
            .and_then(|dir| resolve_path_from_config(dir, config_dir.as_deref()))
        {
            Some(path)
        } else if let Some(path) = legacy_dump_dir
            .as_ref()
            .and_then(|dir| resolve_path_from_config(dir.clone(), config_dir.as_deref()))
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

    let json_enabled = file_dump_json
        .as_ref()
        .and_then(|cfg| cfg.enable)
        .unwrap_or(false);

    let json_dump = if json_enabled {
        let resolved_dir = file_dump_json
            .as_ref()
            .and_then(|cfg| normalize_string(cfg.dir.clone()))
            .and_then(|value| resolve_path_from_config(value, config_dir.as_deref()))
            .unwrap_or_else(|| {
                resolve_path_from_config(DEFAULT_JSON_DUMP_DIR.to_string(), config_dir.as_deref())
                    .unwrap_or_else(|| PathBuf::from(DEFAULT_JSON_DUMP_DIR))
            });

        let frames_filename = file_dump_json
            .as_ref()
            .and_then(|cfg| normalize_string(cfg.frames.clone()))
            .unwrap_or_else(|| DEFAULT_FRAMES_JSON.to_string());
        let segments_filename = file_dump_json
            .as_ref()
            .and_then(|cfg| normalize_string(cfg.segments.clone()))
            .unwrap_or_else(|| DEFAULT_SEGMENTS_JSON.to_string());
        let pretty = file_dump_json
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

    let mut detection_backend = cli.detection_backend;
    if !sources.detection_backend_from_cli {
        if let Some(value) = normalize_string(file_detection_backend) {
            detection_backend = parse_detection_backend(&value, config_path.as_ref())?;
        }
    }

    let mut detection_samples_per_second = cli.detection_samples_per_second;
    if !sources.detection_sps_from_cli {
        if let Some(value) = file_detection_sps {
            if value < 1 {
                return Err(ConfigError::InvalidValue {
                    path: config_path,
                    field: "detection_samples_per_second",
                    value: value.to_string(),
                });
            }
            detection_samples_per_second = value;
        }
    }

    let cli_model = normalize_string(cli.onnx_model.clone());
    let cli_model_present = sources.onnx_model_from_cli && cli_model.is_some();
    let mut onnx_model = cli_model;
    let mut onnx_model_from_cli = cli_model_present;
    if !onnx_model_from_cli {
        if let Some(value) = normalize_string(file_onnx_model) {
            onnx_model = Some(value);
            onnx_model_from_cli = false;
        }
    }

    let mut detection_luma_target = cli.detection_luma_target;
    if !sources.detection_luma_target_from_cli {
        if let Some(value) = file_luma_target {
            detection_luma_target = Some(value);
        }
    }

    let mut detection_luma_delta = cli.detection_luma_delta;
    if !sources.detection_luma_delta_from_cli {
        if let Some(value) = file_luma_delta {
            detection_luma_delta = Some(value);
        }
    }

    let mut decoder_channel_capacity = cli.decoder_channel_capacity;
    if let Some(0) = decoder_channel_capacity {
        return Err(ConfigError::InvalidValue {
            path: None,
            field: "decoder_channel_capacity",
            value: "0".to_string(),
        });
    }
    if !sources.decoder_channel_capacity_from_cli {
        if let Some(value) = file_decoder_channel_capacity {
            if value == 0 {
                return Err(ConfigError::InvalidValue {
                    path: config_path,
                    field: "decoder_channel_capacity",
                    value: value.to_string(),
                });
            }
            decoder_channel_capacity = Some(value);
        }
    }

    Ok(EffectiveSettings {
        backend,
        image_dump,
        json_dump,
        detection_samples_per_second,
        detection_backend,
        onnx_model,
        onnx_model_from_cli,
        config_dir,
        detection_luma_target,
        detection_luma_delta,
        decoder_channel_capacity,
    })
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

fn parse_detection_backend(
    value: &str,
    path: Option<&PathBuf>,
) -> Result<DetectionBackend, ConfigError> {
    DetectionBackend::from_str(value, false).map_err(|_| ConfigError::InvalidValue {
        path: path.cloned(),
        field: "detection_backend",
        value: value.to_string(),
    })
}
