use std::env;
use std::fmt;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::str::FromStr;

#[cfg(feature = "backend-ffmpeg")]
use std::sync::OnceLock;

use crate::core::{DynYPlaneProvider, YPlaneError, YPlaneResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Mock,
    Ffmpeg,
    VideoToolbox,
    OpenH264,
    Mft,
}

impl FromStr for Backend {
    type Err = YPlaneError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "mock" => Ok(Backend::Mock),
            "ffmpeg" => Ok(Backend::Ffmpeg),
            "videotoolbox" => Ok(Backend::VideoToolbox),
            "openh264" => Ok(Backend::OpenH264),
            "mft" => Ok(Backend::Mft),
            other => Err(YPlaneError::configuration(format!(
                "unknown backend '{other}'"
            ))),
        }
    }
}

impl Backend {
    pub fn as_str(&self) -> &'static str {
        match self {
            Backend::Mock => "mock",
            Backend::Ffmpeg => "ffmpeg",
            Backend::VideoToolbox => "videotoolbox",
            Backend::OpenH264 => "openh264",
            Backend::Mft => "mft",
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
fn append_platform_backends(backends: &mut Vec<Backend>) {
    #[cfg(feature = "backend-videotoolbox")]
    {
        backends.push(Backend::VideoToolbox);
    }
    #[cfg(feature = "backend-ffmpeg")]
    {
        if ffmpeg_runtime_available() {
            backends.push(Backend::Ffmpeg);
        }
    }
    #[cfg(feature = "backend-openh264")]
    {
        backends.push(Backend::OpenH264);
    }
}

#[cfg(not(target_os = "macos"))]
fn append_platform_backends(backends: &mut Vec<Backend>) {
    #[cfg(all(feature = "backend-mft", target_os = "windows"))]
    {
        backends.push(Backend::Mft);
    }
    #[cfg(feature = "backend-ffmpeg")]
    {
        if ffmpeg_runtime_available() {
            backends.push(Backend::Ffmpeg);
        }
    }
    #[cfg(feature = "backend-videotoolbox")]
    {
        backends.push(Backend::VideoToolbox);
    }
    #[cfg(feature = "backend-openh264")]
    {
        backends.push(Backend::OpenH264);
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

#[cfg(not(feature = "backend-ffmpeg"))]
fn ffmpeg_runtime_available() -> bool {
    false
}

#[derive(Debug, Clone)]
pub struct Configuration {
    pub backend: Backend,
    pub input: Option<PathBuf>,
    pub channel_capacity: Option<NonZeroUsize>,
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
        }
    }
}

impl Configuration {
    pub fn from_env() -> YPlaneResult<Self> {
        let mut config = Configuration::default();
        if let Ok(backend) = env::var("SUBFAST_BACKEND") {
            config.backend = Backend::from_str(&backend)?;
        }
        if let Ok(path) = env::var("SUBFAST_INPUT") {
            config.input = Some(PathBuf::from(path));
        }
        if let Ok(capacity) = env::var("SUBFAST_CHANNEL_CAPACITY") {
            let parsed: usize = capacity.parse().map_err(|_| {
                YPlaneError::configuration(format!(
                    "failed to parse SUBFAST_CHANNEL_CAPACITY='{capacity}' as a positive integer"
                ))
            })?;
            let Some(value) = NonZeroUsize::new(parsed) else {
                return Err(YPlaneError::configuration(
                    "SUBFAST_CHANNEL_CAPACITY must be greater than zero",
                ));
            };
            config.channel_capacity = Some(value);
        }
        Ok(config)
    }

    pub fn available_backends() -> Vec<Backend> {
        compiled_backends()
    }

    pub fn create_provider(&self) -> YPlaneResult<DynYPlaneProvider> {
        let channel_capacity = self.channel_capacity.map(NonZeroUsize::get);

        match self.backend {
            Backend::Mock => {
                if !github_ci_active() {
                    return Err(YPlaneError::unsupported("mock"));
                }
                return crate::backends::mock::boxed_mock(self.input.clone(), channel_capacity);
            }
            Backend::Ffmpeg => {
                #[cfg(feature = "backend-ffmpeg")]
                {
                    let path = self.input.clone().ok_or_else(|| {
                        YPlaneError::configuration("FFmpeg backend requires SUBFAST_INPUT")
                    })?;
                    return crate::backends::ffmpeg::boxed_ffmpeg(path, channel_capacity);
                }
                #[cfg(not(feature = "backend-ffmpeg"))]
                {
                    return Err(YPlaneError::unsupported("ffmpeg"));
                }
            }
            Backend::VideoToolbox => {
                #[cfg(feature = "backend-videotoolbox")]
                {
                    let path = self.input.clone().ok_or_else(|| {
                        YPlaneError::configuration(
                            "VideoToolbox backend requires SUBFAST_INPUT to be set",
                        )
                    })?;
                    return crate::backends::videotoolbox::boxed_videotoolbox(
                        path,
                        channel_capacity,
                    );
                }
                #[cfg(not(feature = "backend-videotoolbox"))]
                {
                    return Err(YPlaneError::unsupported("videotoolbox"));
                }
            }
            Backend::OpenH264 => {
                #[cfg(feature = "backend-openh264")]
                {
                    let path = self.input.clone().ok_or_else(|| {
                        YPlaneError::configuration(
                            "OpenH264 backend requires SUBFAST_INPUT to be set",
                        )
                    })?;
                    return crate::backends::openh264::boxed_openh264(path, channel_capacity);
                }
                #[cfg(not(feature = "backend-openh264"))]
                {
                    return Err(YPlaneError::unsupported("openh264"));
                }
            }
            Backend::Mft => {
                #[cfg(all(feature = "backend-mft", target_os = "windows"))]
                {
                    let path = self.input.clone().ok_or_else(|| {
                        YPlaneError::configuration("MFT backend requires SUBFAST_INPUT to be set")
                    })?;
                    return crate::backends::mft::boxed_mft(path, channel_capacity);
                }
                #[cfg(not(all(feature = "backend-mft", target_os = "windows")))]
                {
                    return Err(YPlaneError::unsupported("mft"));
                }
            }
        }
    }
}

fn default_backend() -> Backend {
    if github_ci_active() {
        Backend::Mock
    } else if cfg!(all(target_os = "windows", feature = "backend-mft")) {
        Backend::Mft
    } else {
        Backend::Ffmpeg
    }
}

fn github_ci_active() -> bool {
    env::var("GITHUB_ACTIONS")
        .map(|value| !value.is_empty() && value != "false")
        .unwrap_or(false)
}
