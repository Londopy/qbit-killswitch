use anyhow::{bail, Context, Result};
use semver::Version;
use sha2::{Digest, Sha256};

const GITHUB_API_LATEST: &str =
    "https://api.github.com/repos/Londopy/qbit-killswitch/releases/latest";

// ── GitHub API types ──────────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets:   Vec<GithubAsset>,
}

#[derive(Debug, serde::Deserialize)]
struct GithubAsset {
    name:                 String,
    browser_download_url: String,
}

// ── Platform asset selection ──────────────────────────────────────────────────

/// Filename suffix for the portable binary on this platform/arch.
/// Matches the naming convention in release.yml.
fn asset_suffix() -> &'static str {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    { "windows-x64.exe" }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    { "linux-x64" }
    #[cfg(target_os = "macos")]
    { "macos-arm64" }
    // Fallback for any unsupported combination.
    #[cfg(not(any(
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "linux",   target_arch = "x86_64"),
        target_os = "macos",
    )))]
    { "linux-x64" }
}

// ── HTTP client ───────────────────────────────────────────────────────────────

fn build_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(format!("qbit-killswitch/{}", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Building HTTP client")
}

// ── Core API calls ────────────────────────────────────────────────────────────

fn fetch_latest_release() -> Result<GithubRelease> {
    build_client()?
        .get(GITHUB_API_LATEST)
        .send()
        .context("Fetching latest release from GitHub")?
        .json()
        .context("Parsing release JSON")
}

/// Compare the latest GitHub release against the running binary.
/// Returns `Some(version_str)` when a newer release exists, `None` when already current.
pub fn check_for_update() -> Result<Option<String>> {
    let release    = fetch_latest_release()?;
    let remote_str = release.tag_name.trim_start_matches('v');
    let remote     = Version::parse(remote_str)
        .with_context(|| format!("Parsing remote version '{remote_str}'"))?;
    let current    = Version::parse(env!("CARGO_PKG_VERSION"))
        .context("Parsing current version")?;
    if remote > current {
        Ok(Some(remote_str.to_string()))
    } else {
        Ok(None)
    }
}

/// Download the platform binary, verify its SHA-256 against `checksums.txt`,
/// replace the running executable, and restart.
///
/// **On success this function never returns** — it calls `process::exit(0)` after
/// spawning the updated process.
pub fn download_and_apply() -> Result<()> {
    let release = fetch_latest_release()?;
    let suffix  = asset_suffix();
    let client  = build_client()?;

    // Locate the right binary asset.
    let asset = release
        .assets
        .iter()
        .find(|a| a.name.ends_with(suffix))
        .ok_or_else(|| anyhow::anyhow!(
            "No release asset found for platform suffix '{suffix}'"
        ))?;

    // Download binary bytes.
    let bytes = client
        .get(&asset.browser_download_url)
        .send()
        .context("Downloading update binary")?
        .bytes()
        .context("Reading update binary bytes")?;

    // Verify SHA-256 against checksums.txt if it is present in the release.
    if let Some(cs) = release.assets.iter().find(|a| a.name == "checksums.txt") {
        let text = client
            .get(&cs.browser_download_url)
            .send()
            .context("Downloading checksums.txt")?
            .text()
            .context("Reading checksums.txt")?;

        // Format: "<sha256hex>  <filename>\n" (sha256sum output)
        if let Some(expected) = text
            .lines()
            .find(|l| l.contains(&asset.name))
            .and_then(|l| l.split_whitespace().next())
            .map(|s| s.to_lowercase())
        {
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            let actual = hex::encode(hasher.finalize());
            if actual != expected {
                bail!(
                    "SHA-256 mismatch — download may be corrupt.\n\
                     Expected: {expected}\n\
                     Got:      {actual}"
                );
            }
        }
    }

    // Write to a temp file.
    let tmp = if cfg!(target_os = "windows") {
        std::env::temp_dir().join("qbit-killswitch-update.exe")
    } else {
        std::env::temp_dir().join("qbit-killswitch-update")
    };
    std::fs::write(&tmp, &bytes).context("Writing update binary to temp file")?;

    // Make the file executable on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .context("Setting executable bit on update binary")?;
    }

    // Replace the running binary and spawn the updated process.
    self_replace::self_replace(&tmp).context("self_replace failed")?;
    let exe = std::env::current_exe().context("Getting current exe path")?;
    std::process::Command::new(&exe)
        .spawn()
        .context("Spawning updated process")?;
    std::process::exit(0);
}

// ── Background helpers ────────────────────────────────────────────────────────

/// Spawn a background thread to check for updates.
/// The result — `Ok(Some(version))`, `Ok(None)`, or `Err(message)` — is sent to `tx`.
pub fn background_check(
    tx: std::sync::mpsc::Sender<Result<Option<String>, String>>,
) {
    std::thread::spawn(move || {
        let _ = tx.send(check_for_update().map_err(|e| e.to_string()));
    });
}

/// Spawn a background thread to download and apply the update.
/// Returns a receiver that yields `Err(message)` if anything fails.
/// On success, `download_and_apply` calls `process::exit(0)`, so the channel
/// is never fired on the happy path.
pub fn background_apply() -> std::sync::mpsc::Receiver<Result<(), String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Err(e) = download_and_apply() {
            let _ = tx.send(Err(e.to_string()));
        }
        // On success: process::exit(0) inside download_and_apply; tx never fires.
    });
    rx
}
