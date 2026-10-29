use std::sync::OnceLock;
use std::time::Duration;

use serde::de::DeserializeOwned;

use crate::error::{Error, Result};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const USER_AGENT: &str =
    "KestrelLauncher/0.1 (+https://github.com/kestrel-launcher/kestrel)";

pub const PISTON_MANIFEST_URL: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Shared HTTP/2-capable client with connection pooling. One client for the
/// whole process keeps TCP+TLS setup off the critical path after warmup.
pub fn client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            // No total request timeout: large jars/assets legitimately take
            // longer. A read timeout still catches stalled connections.
            .read_timeout(Duration::from_secs(60))
            .pool_max_idle_per_host(32)
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_nodelay(true)
            .http2_adaptive_window(true)
            .gzip(true)
            .build()
            .expect("failed to build HTTP client")
    })
}

pub async fn get_bytes(url: &str) -> Result<bytes::Bytes> {
    let response = client().get(url).timeout(Duration::from_secs(60)).send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::msg(format!("GET {url} failed with HTTP {status}")));
    }
    Ok(response.bytes().await?)
}

pub async fn get_json<T: DeserializeOwned>(url: &str) -> Result<T> {
    let bytes = get_bytes(url).await?;
    serde_json::from_slice(&bytes).map_err(|e| Error::json(url.to_string(), e))
}
