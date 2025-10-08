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
    detection_samples_per_second: Option<u32>,
    detection_backend: Option<String>,
    onnx_model: Option<String>,
}

#[derive(Debug)]
pub struct EffectiveSettings {
    pub backend: Option<String>,
    pub dump_dir: Option<PathBuf>,
    pub dump_format: DumpFormat,
    pub detection_samples_per_second: u32,
    pub detection_backend: DetectionBackend,
    pub onnx_model: Option<String>,
    pub onnx_model_from_cli: bool,
    pub config_dir: Option<PathBuf>,
}

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
        detection_samples_per_second: file_detection_sps,
        detection_backend: file_detection_backend,
        onnx_model: file_onnx_model,
    } = file;

    let mut backend = normalize_string(cli.backend.clone());
    if backend.is_none() {
        backend = normalize_string(file_backend);
    }

    let dump_dir = cli.dump_dir.clone().map(expand_pathbuf).or_else(|| {
        file_dump_dir.and_then(|dir| resolve_path_from_config(dir, config_dir.as_deref()))
    });

    let mut dump_format = cli.dump_format;
    if !sources.dump_format_from_cli {
        if let Some(format_str) = normalize_string(file_dump_format) {
            dump_format = parse_dump_format(&format_str, config_path.as_ref())?;
        }
    }

    let mut detection_backend = cli.detection_backend;
    if !sources.detection_backend_from_cli {
        if let Some(value) = normalize_string(file_detection_backend) {
            detection_backend = parse_detection_backend(&value, config_path.as_ref())?;
        }
    }

    let mut detection_samples_per_second = cli.detection_samples_per_second;
    if !sources.detection_sps_from_cli {
        if let Some(value) = file_detection_sps {
            if value == 0 {
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

    Ok(EffectiveSettings {
        backend,
        dump_dir,
        dump_format,
        detection_samples_per_second,
        detection_backend,
        onnx_model,
        onnx_model_from_cli,
        config_dir,
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
