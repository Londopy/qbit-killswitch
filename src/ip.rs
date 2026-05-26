use std::net::IpAddr;
use std::str::FromStr;
use std::time::Duration;

/// Try each URL in `urls` in order. Return the first plain-text IP that parses,
/// or `None` if every URL fails (timeout, network error, or non-IP body).
pub async fn fetch_ip(client: &reqwest::Client, urls: &[String]) -> Option<IpAddr> {
    for url in urls {
        match try_url(client, url).await {
            Some(ip) => return Some(ip),
            None     => continue,
        }
    }
    None
}

async fn try_url(client: &reqwest::Client, url: &str) -> Option<IpAddr> {
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        client.get(url).send(),
    )
    .await
    .ok()?   // timeout
    .ok()?;  // request error

    if !response.status().is_success() {
        return None;
    }

    let body = tokio::time::timeout(Duration::from_secs(5), response.text())
        .await
        .ok()?
        .ok()?;

    IpAddr::from_str(body.trim()).ok()
}
