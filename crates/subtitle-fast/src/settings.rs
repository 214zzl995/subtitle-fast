use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use directories::ProjectDirs;
use serde::Deserialize;
use subtitle_fast_comparator::ComparatorKind;
use subtitle_fast_types::RoiConfig;
use subtitle_fast_validator::subtitle_detection::{DEFAULT_DELTA, DEFAULT_TARGET};

use crate::cli::{CliArgs, CliSources};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct FileConfig {
    detection: Option<DetectionFileConfig>,
    decoder: Option<DecoderFileConfig>,
    output: Option<OutputFileConfig>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
struct DecoderFileConfig {
    backend: Option<String>,
    channel_capacity: Option<usize>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
struct DetectionFileConfig {
    samples_per_second: Option<u32>,
    target: Option<u8>,
    delta: Option<u8>,
    comparator: Option<String>,
    roi: Option<RoiFileConfig>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
struct RoiFileConfig {
    x: Option<f32>,
    y: Option<f32>,
    width: Option<f32>,
    height: Option<f32>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
struct OutputFileConfig {
    path: Option<PathBuf>,
}

#[derive(Debug)]
pub struct EffectiveSettings {
    pub detection: DetectionSettings,
    pub decoder: DecoderSettings,
    pub output: OutputSettings,
}

#[derive(Debug)]
pub struct ResolvedSettings {
    pub settings: EffectiveSettings,
}

#[derive(Debug, Clone)]
pub struct DetectionSettings {
    pub samples_per_second: u32,
    pub target: u8,
    pub delta: u8,
    pub comparator: Option<ComparatorKind>,
    pub roi: Option<RoiConfig>,
}

#[derive(Debug, Clone, Default)]
pub struct DecoderSettings {
    pub backend: Option<String>,
    pub channel_capacity: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct OutputSettings {
    pub path: Option<PathBuf>,
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

    if let Some(project_path) = project_config_path()
        && project_path.exists()
    {
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
    let FileConfig {
        detection: file_detection,
        decoder: file_decoder,
        output: file_output,
    } = file;

    let detection_cfg = file_detection.unwrap_or_default();
    let decoder_cfg = file_decoder.unwrap_or_default();
    let output_cfg = file_output.unwrap_or_default();

    let detection_samples_per_second = resolve_detection_sps(
        cli.detection_samples_per_second,
        detection_cfg.samples_per_second,
        !sources.detection_sps_from_cli,
        config_path.as_ref(),
    )?;

    let detector_target = resolve_detector_u8(
        cli.detector_target,
        detection_cfg.target,
        !sources.detector_target_from_cli,
        DEFAULT_TARGET,
    )?;
    let detector_delta = resolve_detector_u8(
        cli.detector_delta,
        detection_cfg.delta,
        !sources.detector_delta_from_cli,
        DEFAULT_DELTA,
    )?;

    let comparator_kind = resolve_comparator_kind(
        cli.comparator.clone(),
        detection_cfg.comparator.clone(),
        !sources.comparator_from_cli,
        config_path.as_ref(),
    )?;

    let detection_roi = resolve_detection_roi(
        cli.roi,
        detection_cfg.roi,
        !sources.detector_roi_from_cli,
        config_path.as_ref(),
    )?;

    let decoder_channel_capacity = resolve_decoder_capacity(
        cli.decoder_channel_capacity,
        decoder_cfg.channel_capacity,
        !sources.decoder_channel_capacity_from_cli,
        config_path.as_ref(),
    )?;

    let decoder_backend = normalize_string(cli.backend.clone())
        .or_else(|| normalize_string(decoder_cfg.backend.clone()));

    let decoder_settings = DecoderSettings {
        backend: decoder_backend,
        channel_capacity: decoder_channel_capacity,
    };

    let output_settings = OutputSettings {
        path: cli.output.clone().or(output_cfg.path),
    };

    let settings = EffectiveSettings {
        detection: DetectionSettings {
            samples_per_second: detection_samples_per_second,
            target: detector_target,
            delta: detector_delta,
            comparator: comparator_kind,
            roi: Some(detection_roi),
        },
        decoder: decoder_settings,
        output: output_settings,
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

fn resolve_detection_sps(
    cli_value: u32,
    file_value: Option<u32>,
    use_file: bool,
    config_path: Option<&PathBuf>,
) -> Result<u32, ConfigError> {
    if use_file && let Some(value) = file_value {
        if value < 1 {
            return Err(ConfigError::InvalidValue {
                path: config_path.cloned(),
                field: "detection_samples_per_second",
                value: value.to_string(),
            });
        }
        return Ok(value);
    }
    Ok(cli_value)
}

fn resolve_detector_u8(
    cli_value: Option<u8>,
    file_value: Option<u8>,
    use_file: bool,
    default: u8,
) -> Result<u8, ConfigError> {
    if let Some(value) = cli_value {
        return Ok(value);
    }
    if use_file && let Some(value) = file_value {
        return Ok(value);
    }
    Ok(default)
}

fn full_frame_roi() -> RoiConfig {
    RoiConfig {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    }
}

fn resolve_detection_roi(
    cli_value: Option<RoiConfig>,
    file_value: Option<RoiFileConfig>,
    use_file: bool,
    config_path: Option<&PathBuf>,
) -> Result<RoiConfig, ConfigError> {
    let raw = if let Some(roi) = cli_value {
        Some(roi)
    } else if use_file {
        file_value.map(|roi| RoiConfig {
            x: roi.x.unwrap_or(0.0),
            y: roi.y.unwrap_or(0.0),
            width: roi.width.unwrap_or(0.0),
            height: roi.height.unwrap_or(0.0),
        })
    } else {
        None
    };

    let normalized = match raw {
        Some(roi) => normalize_roi(roi, config_path)?,
        None => None,
    };

    Ok(normalized.unwrap_or_else(full_frame_roi))
}

fn normalize_roi(
    roi: RoiConfig,
    config_path: Option<&PathBuf>,
) -> Result<Option<RoiConfig>, ConfigError> {
    if roi.x < 0.0 || roi.y < 0.0 || roi.width < 0.0 || roi.height < 0.0 {
        return Err(ConfigError::InvalidValue {
            path: config_path.cloned(),
            field: "detection_roi",
            value: format!("{},{},{},{}", roi.x, roi.y, roi.width, roi.height),
        });
    }

    if roi.width == 0.0 || roi.height == 0.0 {
        return Ok(None);
    }

    let x = roi.x.min(1.0);
    let y = roi.y.min(1.0);
    let max_width = (1.0 - x).max(0.0);
    let max_height = (1.0 - y).max(0.0);
    let width = roi.width.min(1.0).min(max_width);
    let height = roi.height.min(1.0).min(max_height);

    if width <= 0.0 || height <= 0.0 {
        return Ok(None);
    }

    Ok(Some(RoiConfig {
        x: round_roi(x),
        y: round_roi(y),
        width: round_roi(width),
        height: round_roi(height),
    }))
}

fn round_roi(value: f32) -> f32 {
    const SCALE: f32 = 1_000_000.0;
    (value * SCALE).round() / SCALE
}

fn resolve_comparator_kind(
    cli_value: Option<String>,
    file_value: Option<String>,
    use_file: bool,
    config_path: Option<&PathBuf>,
) -> Result<Option<ComparatorKind>, ConfigError> {
    let raw = match normalize_string(cli_value) {
        Some(value) => Some(value),
        None => {
            if use_file {
                normalize_string(file_value)
            } else {
                None
            }
        }
    };

    let Some(value) = raw else {
        return Ok(None);
    };

    match ComparatorKind::from_str(&value) {
        Ok(kind) => Ok(Some(kind)),
        Err(_) => Err(ConfigError::InvalidValue {
            path: config_path.cloned(),
            field: "comparator",
            value,
        }),
    }
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
    if use_file && let Some(value) = file_value {
        if value == 0 {
            return Err(ConfigError::InvalidValue {
                path: config_path.cloned(),
                field: "decoder_channel_capacity",
                value: value.to_string(),
            });
        }
        capacity = Some(value);
    }
    Ok(capacity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roi_defaults_to_full_when_missing() {
        let roi = resolve_detection_roi(None, None, true, None).unwrap();
        assert_eq!(roi, full_frame_roi());
    }

    #[test]
    fn zero_sized_roi_falls_back_to_full_frame() {
        let roi = resolve_detection_roi(
            Some(RoiConfig {
                x: 0.5,
                y: 0.5,
                width: 0.0,
                height: 0.0,
            }),
            None,
            false,
            None,
        )
        .unwrap();
        assert_eq!(roi, full_frame_roi());
    }

    #[test]
    fn roi_clamps_to_bounds() {
        let roi = resolve_detection_roi(
            Some(RoiConfig {
                x: 0.9,
                y: 0.1,
                width: 0.5,
                height: 0.95,
            }),
            None,
            false,
            None,
        )
        .unwrap();
        assert_eq!(
            roi,
            RoiConfig {
                x: 0.9,
                y: 0.1,
                width: 0.1,
                height: 0.9
            }
        );
    }

    #[test]
    fn negative_roi_is_invalid() {
        let err = resolve_detection_roi(
            Some(RoiConfig {
                x: -0.1,
                y: 0.0,
                width: 0.2,
                height: 0.2,
            }),
            None,
            false,
            None,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigError::InvalidValue { field, .. } if field == "detection_roi"
        ));
    }

    #[test]
    fn file_roi_defaults_to_full_when_empty() {
        let file_roi = RoiFileConfig {
            x: Some(0.0),
            y: Some(0.0),
            width: Some(0.0),
            height: Some(0.0),
        };
        let roi = resolve_detection_roi(None, Some(file_roi), true, None).unwrap();
        assert_eq!(roi, full_frame_roi());
    }
}
