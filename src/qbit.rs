use anyhow::{bail, Context, Result};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

/// Thin async wrapper around the qBittorrent Web API.
/// A single `reqwest::Client` with a cookie jar is shared across all calls so
/// that the session cookie from `/api/v2/auth/login` persists automatically.
pub struct QbitClient {
    client:   reqwest::Client,
    base_url: String,
    username: String,
    password: String,
}

impl QbitClient {
    pub fn new(base_url: &str, username: &str, password: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .cookie_store(true)
            .timeout(TIMEOUT)
            .build()
            .context("Failed to build HTTP client")?;
        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            username: username.to_string(),
            password: password.to_string(),
        })
    }

    pub fn update_credentials(&mut self, base_url: &str, username: &str, password: &str) {
        self.base_url = base_url.trim_end_matches('/').to_string();
        self.username = username.to_string();
        self.password = password.to_string();
    }

    // ── Auth ──────────────────────────────────────────────────────────────────

    pub async fn login(&self) -> Result<()> {
        let url = format!("{}/api/v2/auth/login", self.base_url);
        let resp = self
            .client
            .post(&url)
            .form(&[("username", &self.username), ("password", &self.password)])
            .send()
            .await
            .context("Login request failed")?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();

        if status == 403 {
            bail!("IP banned by qBittorrent (403)");
        }
        if body.trim() == "Fails." {
            bail!("Invalid credentials — check your username and password");
        }
        if body.trim() != "Ok." {
            bail!("{}", diagnose_unexpected(&body));
        }
        Ok(())
    }

    // ── Torrent control ───────────────────────────────────────────────────────

    pub async fn pause_all(&self) -> Result<()> {
        self.post_with_relogin("/api/v2/torrents/pause", &[("hashes", "all")]).await
    }

    pub async fn resume_all(&self) -> Result<()> {
        self.post_with_relogin("/api/v2/torrents/resume", &[("hashes", "all")]).await
    }

    /// Return the number of torrents currently in an active downloading state.
    pub async fn downloading_count(&self) -> Result<usize> {
        let url = format!("{}/api/v2/torrents/info", self.base_url);
        let resp = self
            .client
            .get(&url)
            .query(&[("filter", "downloading")])
            .send()
            .await
            .context("torrents/info request failed")?;

        if resp.status() == 403 {
            self.login().await?;
            // Retry once after re-login
            let resp2 = self
                .client
                .get(&url)
                .query(&[("filter", "downloading")])
                .send()
                .await
                .context("torrents/info retry failed")?;
            let list: Vec<serde_json::Value> =
                resp2.json().await.context("torrents/info JSON parse failed")?;
            return Ok(list.len());
        }

        let list: Vec<serde_json::Value> =
            resp.json().await.context("torrents/info JSON parse failed")?;
        Ok(list.len())
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    async fn post_with_relogin(&self, path: &str, form: &[(&str, &str)]) -> Result<()> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .client
            .post(&url)
            .form(form)
            .send()
            .await
            .context(format!("POST {path} failed"))?;

        if resp.status() == 403 {
            // Session expired — re-login and retry once
            self.login()
                .await
                .context("Re-login after 403 failed")?;
            let resp2 = self
                .client
                .post(&url)
                .form(form)
                .send()
                .await
                .context(format!("POST {path} retry after re-login failed"))?;
            if !resp2.status().is_success() {
                bail!("POST {path} failed after re-login: HTTP {}", resp2.status());
            }
            return Ok(());
        }

        if !resp.status().is_success() {
            bail!("POST {path} failed: HTTP {}", resp.status());
        }
        Ok(())
    }
}

/// Expose the underlying `reqwest::Client` so that `ip::fetch_ip` can reuse it.
impl QbitClient {
    pub fn inner(&self) -> &reqwest::Client {
        &self.client
    }
}

/// Produce a human-readable diagnosis when the API returns something unexpected.
fn diagnose_unexpected(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.starts_with('<') {
        return "Got an HTML page instead of the qBittorrent API — \
             check the URL points to the Web UI (e.g. http://127.0.0.1:8080). \
             Make sure the Web UI is enabled in qBittorrent → Tools → Options → Web UI."
            .to_string();
    }
    if trimmed.is_empty() {
        return "Empty response from server — is the qBittorrent Web UI enabled and reachable?".into();
    }
    let snippet: String = trimmed.chars().take(120).collect();
    format!("Unexpected response from qBittorrent: {snippet:?}")
}

/// One-shot test: try to log in with the given credentials. Returns Ok(()) or an error string.
pub async fn test_connection(base_url: &str, username: &str, password: &str) -> Result<()> {
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .timeout(TIMEOUT)
        .build()
        .context("HTTP client build failed")?;

    let url = format!("{}/api/v2/auth/login", base_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .form(&[("username", username), ("password", password)])
        .send()
        .await
        .context("Connection failed — check the URL and that qBittorrent is running")?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if status == 403 {
        bail!("IP banned by qBittorrent — restart qBittorrent to clear the ban");
    }
    if body.trim() == "Fails." {
        bail!("Invalid credentials — check your username and password");
    }
    if body.trim() != "Ok." {
        bail!("{}", diagnose_unexpected(&body));
    }
    Ok(())
}
