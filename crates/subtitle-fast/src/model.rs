use std::fmt;
use std::path::{Path, PathBuf};

use directories::{BaseDirs, ProjectDirs};
use futures_util::StreamExt;
use hex::encode as hex_encode;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::{StatusCode, Url};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;

#[derive(Debug)]
pub enum ModelError {
    CacheDirUnavailable,
    Download {
        url: Url,
        status: StatusCode,
    },
    Http {
        url: Url,
        source: reqwest::Error,
    },
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    InvalidUrl {
        value: String,
    },
    InvalidFileUrl {
        value: String,
    },
    LocalModelNotFound {
        path: PathBuf,
    },
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelError::CacheDirUnavailable => {
                write!(f, "unable to determine cache directory for models")
            }
            ModelError::Download { url, status } => {
                write!(f, "failed to download {url} (status {status})")
            }
            ModelError::Http { url, source } => {
                write!(f, "failed to download {url}: {source}")
            }
            ModelError::Io { path, source } => {
                write!(f, "filesystem error at {}: {}", path.display(), source)
            }
            ModelError::InvalidUrl { value } => {
                write!(f, "invalid model URI '{value}'")
            }
            ModelError::InvalidFileUrl { value } => {
                write!(f, "model URI '{value}' is not a valid file path")
            }
            ModelError::LocalModelNotFound { path } => {
                write!(f, "model file {} does not exist", path.display())
            }
        }
    }
}

impl std::error::Error for ModelError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ModelError::Http { source, .. } => Some(source),
            ModelError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub async fn resolve_model_path(
    model: Option<&str>,
    from_cli: bool,
    config_dir: Option<&Path>,
) -> Result<Option<PathBuf>, ModelError> {
    let Some(value) = model.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let base_dir = if from_cli { None } else { config_dir };
    match ModelLocator::parse(value, base_dir)? {
        ModelLocator::Local(path) => {
            let exists = fs::try_exists(&path)
                .await
                .map_err(|source| ModelError::Io {
                    path: path.clone(),
                    source,
                })?;
            if !exists {
                return Err(ModelError::LocalModelNotFound { path });
            }
            Ok(Some(path))
        }
        ModelLocator::Remote(url) => {
            let cache = ModelCache::new()?;
            let target = cache.cached_path(&url);
            if !fs::try_exists(&target)
                .await
                .map_err(|source| ModelError::Io {
                    path: target.clone(),
                    source,
                })?
            {
                cache.ensure_dir().await?;
                download_model(&url, &target).await?;
            }
            Ok(Some(target))
        }
    }
}

enum ModelLocator {
    Local(PathBuf),
    Remote(Url),
}

impl ModelLocator {
    fn parse(value: &str, base_dir: Option<&Path>) -> Result<Self, ModelError> {
        if value.starts_with("http://") || value.starts_with("https://") {
            let url = Url::parse(value).map_err(|_| ModelError::InvalidUrl {
                value: value.to_string(),
            })?;
            return Ok(ModelLocator::Remote(url));
        }
        if value.starts_with("file://") {
            let url = Url::parse(value).map_err(|_| ModelError::InvalidUrl {
                value: value.to_string(),
            })?;
            let path = url.to_file_path().map_err(|_| ModelError::InvalidFileUrl {
                value: value.to_string(),
            })?;
            return Ok(ModelLocator::Local(path));
        }
        Ok(ModelLocator::Local(resolve_local_path(value, base_dir)))
    }
}

struct ModelCache {
    root: PathBuf,
}

impl ModelCache {
    fn new() -> Result<Self, ModelError> {
        let dirs = ProjectDirs::from("rs", "subtitle-fast", "subtitle-fast")
            .ok_or(ModelError::CacheDirUnavailable)?;
        Ok(Self {
            root: dirs.data_local_dir().join("models"),
        })
    }

    fn cached_path(&self, url: &Url) -> PathBuf {
        self.root.join(build_cache_file_name(url))
    }

    async fn ensure_dir(&self) -> Result<(), ModelError> {
        fs::create_dir_all(&self.root)
            .await
            .map_err(|source| ModelError::Io {
                path: self.root.clone(),
                source,
            })
    }
}

async fn download_model(url: &Url, dest: &Path) -> Result<(), ModelError> {
    let response = reqwest::get(url.clone())
        .await
        .map_err(|source| ModelError::Http {
            url: url.clone(),
            source,
        })?;
    if !response.status().is_success() {
        return Err(ModelError::Download {
            url: url.clone(),
            status: response.status(),
        });
    }
    let mut progress = DownloadProgress::new(url, response.content_length());
    let temp_path = dest.with_extension("download");
    let mut file = fs::File::create(&temp_path)
        .await
        .map_err(|source| ModelError::Io {
            path: temp_path.clone(),
            source,
        })?;
    let mut stream = response.bytes_stream();
    let mut downloaded = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|source| ModelError::Http {
            url: url.clone(),
            source,
        })?;
        file.write_all(&chunk)
            .await
            .map_err(|source| ModelError::Io {
                path: temp_path.clone(),
                source,
            })?;
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        progress.set_position(downloaded);
    }
    file.flush().await.map_err(|source| ModelError::Io {
        path: temp_path.clone(),
        source,
    })?;
    drop(file);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|source| ModelError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
    }
    fs::rename(&temp_path, dest)
        .await
        .map_err(|source| ModelError::Io {
            path: dest.to_path_buf(),
            source,
        })?;
    progress.finish();
    Ok(())
}

struct DownloadProgress {
    bar: Option<ProgressBar>,
}

impl DownloadProgress {
    fn new(url: &Url, total: Option<u64>) -> Self {
        let bar = total.map(|len| {
            let bar = ProgressBar::new(len);
            bar.set_style(
                ProgressStyle::with_template("{msg} {wide_bar} {bytes}/{total_bytes} ({eta})")
                    .unwrap(),
            );
            bar.set_message(format!("downloading model from {url}"));
            bar
        });
        Self { bar }
    }

    fn set_position(&self, pos: u64) {
        if let Some(bar) = &self.bar {
            bar.set_position(pos);
        }
    }

    fn finish(&mut self) {
        if let Some(bar) = self.bar.take() {
            bar.finish_and_clear();
        }
    }
}

impl Drop for DownloadProgress {
    fn drop(&mut self) {
        if let Some(bar) = self.bar.take() {
            bar.abandon();
        }
    }
}

fn resolve_local_path(value: &str, base_dir: Option<&Path>) -> PathBuf {
    let expanded = expand_home(value);
    if expanded.is_absolute() || base_dir.is_none() {
        expanded
    } else {
        base_dir.unwrap().join(expanded)
    }
}

fn expand_home(value: &str) -> PathBuf {
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

fn build_cache_file_name(url: &Url) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(url.as_str().as_bytes());
    let hash = hex_encode(hasher.finalize());
    let short_hash = &hash[..12];

    let last_segment = url
        .path_segments()
        .and_then(|segments| segments.filter(|s| !s.is_empty()).last())
        .unwrap_or("model.bin");

    let (raw_stem, raw_ext) = split_name(last_segment);
    let stem = sanitize_component(raw_stem, "model");
    let ext = raw_ext
        .map(|ext| sanitize_component(ext, "bin"))
        .unwrap_or_else(|| "bin".to_string());

    PathBuf::from(format!("{stem}-{short_hash}.{ext}"))
}

fn split_name(name: &str) -> (&str, Option<&str>) {
    match name.rfind('.') {
        Some(idx) if idx > 0 && idx + 1 < name.len() => (&name[..idx], Some(&name[idx + 1..])),
        _ => (name, None),
    }
}

fn sanitize_component(value: &str, default: &str) -> String {
    let mut sanitized: String = value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    sanitized = sanitized.trim_matches('-').to_string();
    if sanitized.is_empty() {
        default.to_string()
    } else {
        sanitized
    }
}
