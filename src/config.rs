use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const KEYCHAIN_SERVICE: &str = "qbit-killswitch";

// ── Serde defaults ────────────────────────────────────────────────────────────

fn default_true() -> bool { true }
fn default_poll_secs() -> u64 { 10 }
fn default_fail_threshold() -> u32 { 2 }
fn default_qbit_url() -> String { "http://127.0.0.1:8080".to_string() }
fn default_admin() -> String { "admin".to_string() }
pub fn default_ip_check_urls() -> Vec<String> {
    vec![
        "https://ifconfig.me/ip".to_string(),
        "https://api.ipify.org".to_string(),
        "https://checkip.amazonaws.com".to_string(),
    ]
}

// ── Config ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_qbit_url")]
    pub qbit_url: String,

    #[serde(default = "default_admin")]
    pub qbit_user: String,

    /// Plaintext password. Blank when `use_keychain = true` and keychain is working.
    #[serde(default)]
    pub qbit_pass: String,

    #[serde(default = "default_true")]
    pub use_keychain: bool,

    /// Legacy single-URL field. Deserialized if present; never serialized.
    #[serde(default, skip_serializing, rename = "ip_check_url")]
    pub legacy_ip_check_url: Option<String>,

    #[serde(default = "default_ip_check_urls")]
    pub ip_check_urls: Vec<String>,

    #[serde(default = "default_poll_secs")]
    pub poll_secs: u64,

    #[serde(default = "default_fail_threshold")]
    pub fail_threshold: u32,

    #[serde(default)]
    pub launch_at_startup: bool,

    #[serde(default = "default_true")]
    pub minimize_to_tray: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            qbit_url: default_qbit_url(),
            qbit_user: default_admin(),
            qbit_pass: String::new(),
            use_keychain: true,
            legacy_ip_check_url: None,
            ip_check_urls: default_ip_check_urls(),
            poll_secs: 10,
            fail_threshold: 2,
            launch_at_startup: false,
            minimize_to_tray: true,
        }
    }
}

impl Config {
    // ── Persistence ───────────────────────────────────────────────────────────

    pub fn config_path() -> PathBuf {
        #[cfg(target_os = "windows")]
        {
            let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
            PathBuf::from(base).join("qbit-killswitch").join("config.toml")
        }
        #[cfg(target_os = "macos")]
        {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("qbit-killswitch")
                .join("config.toml")
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let base = std::env::var("XDG_CONFIG_HOME")
                .unwrap_or_else(|_| {
                    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                    format!("{}/.config", home)
                });
            PathBuf::from(base).join("qbit-killswitch").join("config.toml")
        }
    }

    /// Load from disk. Returns (config, optional_error_message).
    /// On any failure, returns default config plus a description of what went wrong.
    pub fn load() -> (Self, Option<String>) {
        let path = Self::config_path();
        if !path.exists() {
            return (Self::default(), None);
        }
        match std::fs::read_to_string(&path) {
            Err(e) => (Self::default(), Some(format!("Could not read config: {e}"))),
            Ok(text) => match toml::from_str::<Self>(&text) {
                Err(e) => (Self::default(), Some(format!("Config parse error: {e}"))),
                Ok(mut cfg) => {
                    cfg.normalize();
                    (cfg, None)
                }
            },
        }
    }

    /// Write to disk. Keychain password is NOT written (caller handles that).
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .context("Could not create config directory")?;
        }
        let text = toml::to_string_pretty(self).context("Could not serialize config")?;
        std::fs::write(&path, text).context("Could not write config file")
    }

    /// Apply constraints and migrate legacy fields in-place.
    pub fn normalize(&mut self) {
        self.poll_secs = self.poll_secs.max(5);
        self.fail_threshold = self.fail_threshold.max(1);

        // Promote legacy ip_check_url → ip_check_urls
        if let Some(url) = self.legacy_ip_check_url.take() {
            if !url.is_empty() {
                if self.ip_check_urls.is_empty() {
                    self.ip_check_urls.push(url);
                } else if !self.ip_check_urls.contains(&url) {
                    // Only prepend if it isn't already there
                    self.ip_check_urls.insert(0, url);
                }
            }
        }

        if self.ip_check_urls.is_empty() {
            self.ip_check_urls = default_ip_check_urls();
        }
    }

    // ── Validation (used by the GUI Save button) ──────────────────────────────

    /// Returns a list of (field_name, error_message) pairs. Empty = valid.
    pub fn validate(&self) -> Vec<(&'static str, String)> {
        let mut errors = Vec::new();

        if url::Url::parse(&self.qbit_url).is_err() {
            errors.push(("qbit_url", format!("'{}' is not a valid URL", self.qbit_url)));
        }

        if self.poll_secs < 5 {
            errors.push(("poll_secs", "Poll interval must be at least 5 seconds".into()));
        }

        if self.fail_threshold < 1 {
            errors.push(("fail_threshold", "Fail threshold must be at least 1".into()));
        }

        if self.ip_check_urls.is_empty() {
            errors.push(("ip_check_urls", "At least one IP check URL is required".into()));
        }

        for (i, u) in self.ip_check_urls.iter().enumerate() {
            if url::Url::parse(u).is_err() {
                errors.push(("ip_check_urls", format!("URL #{} is invalid: {u}", i + 1)));
            }
        }

        errors
    }
}

// ── Keychain helpers ──────────────────────────────────────────────────────────

pub fn keychain_get(user: &str) -> Result<String> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, user)?;
    entry.get_password().context("Keychain read failed")
}

pub fn keychain_set(user: &str, password: &str) -> Result<()> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, user)?;
    entry.set_password(password).context("Keychain write failed")
}

pub fn keychain_delete(user: &str) -> Result<()> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, user)?;
    // delete_credential is idempotent — ignore "not found" errors
    let _ = entry.delete_credential();
    Ok(())
}

/// Resolve the runtime password. Returns (password, keychain_warning).
/// `keychain_warning = true` means we wanted the keychain but it failed.
pub fn resolve_password(config: &Config) -> (String, bool) {
    if config.use_keychain {
        match keychain_get(&config.qbit_user) {
            Ok(pass) => (pass, false),
            Err(_)   => (config.qbit_pass.clone(), !config.qbit_pass.is_empty()),
        }
    } else {
        (config.qbit_pass.clone(), false)
    }
}

// ── Auto-launch helper ────────────────────────────────────────────────────────

/// Enable or disable auto-launch. Silently logs errors rather than propagating.
pub fn set_auto_launch(enable: bool) -> Result<()> {
    let exe = std::env::current_exe().context("Could not get current exe path")?;
    let exe_str = exe.to_string_lossy();
    let al = auto_launch::AutoLaunch::new("qbit-killswitch", &exe_str, &[] as &[&str]);
    if enable {
        al.enable().context("Could not enable auto-launch")?;
    } else {
        al.disable().context("Could not disable auto-launch")?;
    }
    Ok(())
}
