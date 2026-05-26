use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;

// ── App state ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    Starting,
    Nominal,
    Degraded,
    Paused,
    Recovery,
}

impl AppState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Starting => "STARTING",
            Self::Nominal  => "NOMINAL",
            Self::Degraded => "DEGRADED",
            Self::Paused   => "PAUSED",
            Self::Recovery => "RECOVERING",
        }
    }

    /// RGB tuple used by the GUI to paint dots and generate tray icons.
    pub fn rgb(self) -> (u8, u8, u8) {
        match self {
            Self::Starting => (150, 150, 150),
            Self::Nominal  => (40,  200, 40),
            Self::Degraded => (240, 195, 0),
            Self::Paused   => (220, 40,  40),
            Self::Recovery => (0,   210, 210),
        }
    }
}

// ── Commands: GUI -> monitor ──────────────────────────────────────────────────

#[derive(Debug)]
pub enum MonitorCommand {
    /// Re-detect the VPN IP and resume if currently paused/degraded.
    RedetectVpnIp,
    /// Force-pause all torrents regardless of IP state.
    ManualPause,
    /// Force-resume all torrents (transitions to Nominal).
    ManualResume,
    /// Apply a new config after the user saves settings.
    ReloadConfig {
        config:   crate::config::Config,
        password: String,
    },
}

// ── Shared state (monitor writes, GUI reads) ──────────────────────────────────

pub struct SharedState {
    pub app_state:         AppState,
    pub vpn_ip:            Option<IpAddr>,
    pub fail_streak:       u32,
    pub start_fail_streak: u32,
    /// True when use_keychain=true but keychain was unavailable at startup.
    pub keychain_warning:  bool,
    /// Transient status message shown in the header for 3 seconds.
    pub transient_message: Option<(String, Instant)>,
    /// Result of the last "Re-detect VPN IP" action.
    pub redetect_result:   Option<Result<IpAddr, String>>,
    /// Set by the App so the monitor can trigger GUI repaints.
    pub egui_ctx:          Option<egui::Context>,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            app_state:         AppState::Starting,
            vpn_ip:            None,
            fail_streak:       0,
            start_fail_streak: 0,
            keychain_warning:  false,
            transient_message: None,
            redetect_result:   None,
            egui_ctx:          None,
        }
    }
}

impl SharedState {
    pub fn set_transient(&mut self, msg: impl Into<String>) {
        self.transient_message = Some((msg.into(), Instant::now()));
        self.request_repaint();
    }

    pub fn request_repaint(&self) {
        if let Some(ctx) = &self.egui_ctx {
            ctx.request_repaint();
        }
    }
}

// ── Type aliases ──────────────────────────────────────────────────────────────

pub type SharedStateHandle = Arc<Mutex<SharedState>>;
pub type CommandSender     = tokio::sync::mpsc::UnboundedSender<MonitorCommand>;
pub type CommandReceiver   = tokio::sync::mpsc::UnboundedReceiver<MonitorCommand>;
