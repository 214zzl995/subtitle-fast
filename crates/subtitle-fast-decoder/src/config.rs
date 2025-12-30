use std::env;
use std::fmt;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::str::FromStr;

#[cfg(feature = "backend-ffmpeg")]
use std::sync::OnceLock;

use crate::core::{DecoderError, DecoderProvider, DecoderResult, DynDecoderProvider};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Mock,
    #[cfg(feature = "backend-ffmpeg")]
    FFmpeg,
    #[cfg(all(feature = "backend-videotoolbox", target_os = "macos"))]
    VideoToolbox,
    #[cfg(all(feature = "backend-dxva", target_os = "windows"))]
    Dxva,
    #[cfg(all(feature = "backend-mft", target_os = "windows"))]
    Mft,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Nv12,
    CVPixelBuffer,
}

impl OutputFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            OutputFormat::Nv12 => "nv12",
            OutputFormat::CVPixelBuffer => "cvpixelbuffer",
        }
    }
}

impl Default for OutputFormat {
    fn default() -> Self {
        Self::Nv12
    }
}

impl FromStr for Backend {
    type Err = DecoderError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "mock" => Ok(Backend::Mock),
            #[cfg(feature = "backend-ffmpeg")]
            "ffmpeg" => Ok(Backend::FFmpeg),
            #[cfg(all(feature = "backend-videotoolbox", target_os = "macos"))]
            "videotoolbox" => Ok(Backend::VideoToolbox),
            #[cfg(all(feature = "backend-dxva", target_os = "windows"))]
            "dxva" => Ok(Backend::Dxva),
            #[cfg(all(feature = "backend-mft", target_os = "windows"))]
            "mft" => Ok(Backend::Mft),
            other => Err(DecoderError::configuration(format!(
                "unknown backend '{other}'"
            ))),
        }
    }
}

impl Backend {
    pub fn as_str(&self) -> &'static str {
        match self {
            Backend::Mock => "mock",
            #[cfg(feature = "backend-ffmpeg")]
            Backend::FFmpeg => "ffmpeg",
            #[cfg(all(feature = "backend-videotoolbox", target_os = "macos"))]
            Backend::VideoToolbox => "videotoolbox",
            #[cfg(all(feature = "backend-dxva", target_os = "windows"))]
            Backend::Dxva => "dxva",
            #[cfg(all(feature = "backend-mft", target_os = "windows"))]
            Backend::Mft => "mft",
            #[allow(unreachable_patterns)]
            _ => "unsupported",
        }
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

fn compiled_backends() -> Vec<Backend> {
    let mut backends = Vec::new();
    if github_ci_active() {
        backends.push(Backend::Mock);
    }
    append_platform_backends(&mut backends);
    backends
}

#[cfg(target_os = "macos")]
fn append_platform_backends(_backends: &mut Vec<Backend>) {
    #[cfg(feature = "backend-videotoolbox")]
    {
        _backends.push(Backend::VideoToolbox);
    }
    #[cfg(feature = "backend-ffmpeg")]
    {
        if ffmpeg_runtime_available() {
            _backends.push(Backend::FFmpeg);
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn append_platform_backends(backends: &mut Vec<Backend>) {
    #[cfg(all(feature = "backend-dxva", target_os = "windows"))]
    {
        backends.push(Backend::Dxva);
    }
    #[cfg(all(feature = "backend-mft", target_os = "windows"))]
    {
        backends.push(Backend::Mft);
    }
    #[cfg(feature = "backend-ffmpeg")]
    {
        if ffmpeg_runtime_available() {
            backends.push(Backend::FFmpeg);
        }
    }
}

#[cfg(feature = "backend-ffmpeg")]
fn ffmpeg_runtime_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| match ffmpeg_next::init() {
        Ok(()) => true,
        Err(err) => {
            eprintln!("ffmpeg backend disabled: failed to initialize libraries ({err})");
            false
        }
    })
}

#[derive(Debug, Clone)]
pub struct Configuration {
    pub backend: Backend,
    pub input: Option<PathBuf>,
    pub channel_capacity: Option<NonZeroUsize>,
    pub output_format: OutputFormat,
    pub start_frame: Option<u64>,
}

impl Default for Configuration {
    fn default() -> Self {
        let backend = compiled_backends()
            .into_iter()
            .next()
            .unwrap_or_else(default_backend);
        Self {
            backend,
            input: None,
            channel_capacity: None,
            output_format: OutputFormat::Nv12,
            start_frame: None,
        }
    }
}

impl Configuration {
    pub fn from_env() -> DecoderResult<Self> {
        let mut config = Configuration::default();
        if let Ok(backend) = env::var("SUBFAST_BACKEND") {
            config.backend = Backend::from_str(&backend)?;
        }
        if let Ok(path) = env::var("SUBFAST_INPUT") {
            config.input = Some(PathBuf::from(path));
        }
        if let Ok(capacity) = env::var("SUBFAST_CHANNEL_CAPACITY") {
            let parsed: usize = capacity.parse().map_err(|_| {
                DecoderError::configuration(format!(
                    "failed to parse SUBFAST_CHANNEL_CAPACITY='{capacity}' as a positive integer"
                ))
            })?;
            let Some(value) = NonZeroUsize::new(parsed) else {
                return Err(DecoderError::configuration(
                    "SUBFAST_CHANNEL_CAPACITY must be greater than zero",
                ));
            };
            config.channel_capacity = Some(value);
        }
        if let Ok(start_frame) = env::var("SUBFAST_START_FRAME") {
            let parsed: u64 = start_frame.parse().map_err(|_| {
                DecoderError::configuration(format!(
                    "failed to parse SUBFAST_START_FRAME='{start_frame}' as a non-negative integer"
                ))
            })?;
            config.start_frame = Some(parsed);
        }
        Ok(config)
    }

    pub fn available_backends() -> Vec<Backend> {
        compiled_backends()
    }

    pub fn create_provider(&self) -> DecoderResult<DynDecoderProvider> {
        self.validate_output_format()?;

        match self.backend {
            Backend::Mock => {
                if !github_ci_active() {
                    Err(DecoderError::unsupported("mock"))
                } else {
                    Ok(Box::new(crate::backends::mock::MockProvider::new(self)?))
                }
            }
            #[cfg(feature = "backend-ffmpeg")]
            Backend::FFmpeg => Ok(Box::new(crate::backends::ffmpeg::FFmpegProvider::new(self)?)),
            #[cfg(all(feature = "backend-videotoolbox", target_os = "macos"))]
            Backend::VideoToolbox => Ok(Box::new(crate::backends::videotoolbox::VideoToolboxProvider::new(self)?)),
            #[cfg(all(feature = "backend-dxva", target_os = "windows"))]
            Backend::Dxva => Ok(Box::new(crate::backends::dxva::DxvaProvider::new(self)?)),
            #[cfg(all(feature = "backend-mft", target_os = "windows"))]
            Backend::Mft => Ok(Box::new(crate::backends::mft::MftProvider::new(self)?)),
            #[allow(unreachable_patterns)]
            other => Err(DecoderError::unsupported(other.as_str())),
        }
    }
}

impl Configuration {
    fn validate_output_format(&self) -> DecoderResult<()> {
        match self.output_format {
            OutputFormat::Nv12 => Ok(()),
            OutputFormat::CVPixelBuffer => {
                #[cfg(all(feature = "backend-videotoolbox", target_os = "macos"))]
                {
                    if self.backend == Backend::VideoToolbox {
                        return Ok(());
                    }
                }

                Err(DecoderError::configuration(format!(
                    "output format '{}' is only supported by videotoolbox backend (selected: {})",
                    self.output_format.as_str(),
                    self.backend.as_str()
                )))
            }
        }
    }
}

fn default_backend() -> Backend {
    if github_ci_active() {
        return Backend::Mock;
    }
    #[cfg(feature = "backend-ffmpeg")]
    return Backend::FFmpeg;

    #[allow(unreachable_code)]
    Backend::Mock
}

fn github_ci_active() -> bool {
    env::var("GITHUB_ACTIONS")
        .map(|value| !value.is_empty() && value != "false")
        .unwrap_or(false)
}
