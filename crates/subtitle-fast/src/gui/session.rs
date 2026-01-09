use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::SharedString;

use crate::gui::components::DetectionHandle;

pub type SessionId = u64;

#[derive(Clone)]
pub struct VideoSession {
    pub id: SessionId,
    pub path: PathBuf,
    pub label: SharedString,
    pub detection: DetectionHandle,
    pub last_timestamp: Option<Duration>,
    pub last_frame_index: Option<u64>,
}

#[derive(Default)]
struct SessionStore {
    next_id: SessionId,
    active_id: Option<SessionId>,
    sessions: Vec<VideoSession>,
}

#[derive(Clone, Default)]
pub struct SessionHandle {
    inner: Arc<Mutex<SessionStore>>,
}

impl SessionHandle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_session(&self, path: PathBuf, detection: DetectionHandle) -> SessionId {
        let label = session_label(&path);
        let mut store = self.inner.lock().expect("session store mutex poisoned");
        store.next_id = store.next_id.saturating_add(1).max(1);
        let id = store.next_id;
        store.sessions.push(VideoSession {
            id,
            path,
            label,
            detection,
            last_timestamp: None,
            last_frame_index: None,
        });
        id
    }

    pub fn sessions_snapshot(&self) -> Vec<VideoSession> {
        let store = self.inner.lock().expect("session store mutex poisoned");
        store.sessions.clone()
    }

    pub fn session(&self, id: SessionId) -> Option<VideoSession> {
        let store = self.inner.lock().expect("session store mutex poisoned");
        store
            .sessions
            .iter()
            .find(|session| session.id == id)
            .cloned()
    }

    pub fn set_active(&self, id: SessionId) {
        let mut store = self.inner.lock().expect("session store mutex poisoned");
        store.active_id = Some(id);
    }

    pub fn active_id(&self) -> Option<SessionId> {
        let store = self.inner.lock().expect("session store mutex poisoned");
        store.active_id
    }

    pub fn update_playback(
        &self,
        id: SessionId,
        timestamp: Option<Duration>,
        frame_index: Option<u64>,
    ) {
        let mut store = self.inner.lock().expect("session store mutex poisoned");
        if let Some(session) = store.sessions.iter_mut().find(|session| session.id == id) {
            if timestamp.is_some() {
                session.last_timestamp = timestamp;
            }
            if frame_index.is_some() {
                session.last_frame_index = frame_index;
            }
        }
    }
}

fn session_label(path: &PathBuf) -> SharedString {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned);
    let label = file_name.unwrap_or_else(|| path.to_string_lossy().to_string());
    SharedString::from(label)
}
