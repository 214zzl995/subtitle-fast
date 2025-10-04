use std::env;
use std::path::PathBuf;
use std::str::FromStr;

use crate::core::{DynYPlaneProvider, YPlaneError, YPlaneResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    Ffmpeg,
    VideoToolbox,
    OpenH264,
    GStreamer,
}

impl FromStr for Backend {
    type Err = YPlaneError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "ffmpeg" => Ok(Backend::Ffmpeg),
            "videotoolbox" => Ok(Backend::VideoToolbox),
            "openh264" => Ok(Backend::OpenH264),
            "gstreamer" => Ok(Backend::GStreamer),
            other => Err(YPlaneError::configuration(format!(
                "unknown backend '{other}'"
            ))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Configuration {
    pub backend: Backend,
    pub input: Option<PathBuf>,
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            input: None,
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
        Ok(config)
    }

    pub fn create_provider(&self) -> YPlaneResult<DynYPlaneProvider> {
        match self.backend {
            Backend::Ffmpeg => {
                #[cfg(feature = "backend-ffmpeg")]
                {
                    let path = self.input.clone().ok_or_else(|| {
                        YPlaneError::configuration("FFmpeg backend requires SUBFAST_INPUT")
                    })?;
                    return crate::backends::ffmpeg::boxed_ffmpeg(path);
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
                    return crate::backends::videotoolbox::boxed_videotoolbox(path);
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
                    return crate::backends::openh264::boxed_openh264(path);
                }
                #[cfg(not(feature = "backend-openh264"))]
                {
                    return Err(YPlaneError::unsupported("openh264"));
                }
            }
            Backend::GStreamer => {
                #[cfg(feature = "backend-gstreamer")]
                {
                    let path = self.input.clone().ok_or_else(|| {
                        YPlaneError::configuration(
                            "GStreamer backend requires SUBFAST_INPUT to be set",
                        )
                    })?;
                    return crate::backends::gstreamer::boxed_gstreamer(path);
                }
                #[cfg(not(feature = "backend-gstreamer"))]
                {
                    return Err(YPlaneError::unsupported("gstreamer"));
                }
            }
        }
    }
}

fn default_backend() -> Backend {
    if cfg!(feature = "backend-ffmpeg") {
        Backend::Ffmpeg
    } else if cfg!(feature = "backend-videotoolbox") {
        Backend::VideoToolbox
    } else if cfg!(feature = "backend-openh264") {
        Backend::OpenH264
    } else if cfg!(feature = "backend-gstreamer") {
        Backend::GStreamer
    } else {
        Backend::Ffmpeg
    }
}
