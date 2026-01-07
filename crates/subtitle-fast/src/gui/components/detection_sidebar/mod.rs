pub mod controls;

pub use controls::DetectionControls;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetectionRunState {
    Idle,
    Running,
    Paused,
}

impl DetectionRunState {
    pub fn is_running(self) -> bool {
        matches!(self, Self::Running | Self::Paused)
    }

    pub fn is_paused(self) -> bool {
        matches!(self, Self::Paused)
    }
}
