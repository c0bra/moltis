//! HTTP/SSE transport for remote MCP servers (Streamable HTTP transport).
//!
//! Uses HTTP POST for JSON-RPC requests and GET for server-initiated SSE events.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use {
    anyhow::{Context, Result, bail},
    reqwest::Client,
    tokio::sync::{Mutex, oneshot},
    tracing::{debug, warn},
};

use crate::{
    traits::McpTransport,
    types::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse},
};

/// HTTP/SSE-based transport for a remote MCP server.
pub struct SseTransport {
    client: Client,
    url: String,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<JsonRpcResponse>>>>,
    next_id: AtomicU64,
}

impl SseTransport {
    /// Create a new SSE transport pointing at the given MCP server URL.
    pub fn new(url: &str) -> Result<Arc<Self>> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .context("failed to build HTTP client for SSE transport")?;

        Ok(Arc::new(Self {
            client,
            url: url.to_string(),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
        }))
    }
}

#[async_trait::async_trait]
impl McpTransport for SseTransport {
    async fn request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = JsonRpcRequest::new(id, method, params);

        debug!(method = %method, id = %id, url = %self.url, "SSE client -> server");

        let http_resp = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .json(&req)
            .send()
            .await
            .with_context(|| format!("SSE POST to '{}' for '{method}' failed", self.url))?;

        if !http_resp.status().is_success() {
            let status = http_resp.status();
            let body = http_resp.text().await.unwrap_or_default();
            bail!("MCP SSE server returned HTTP {status} for '{method}': {body}",);
        }

        let resp: JsonRpcResponse = http_resp
            .json()
            .await
            .with_context(|| format!("failed to parse JSON-RPC response for '{method}'"))?;

        if let Some(ref err) = resp.error {
            bail!(
                "MCP SSE error on '{method}': code={} message={}",
                err.code,
                err.message
            );
        }

        Ok(resp)
    }

    async fn notify(&self, method: &str, params: Option<serde_json::Value>) -> Result<()> {
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params,
        };

        debug!(method = %method, url = %self.url, "SSE client -> server (notification)");

        let http_resp = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .json(&notif)
            .send()
            .await
            .with_context(|| {
                format!(
                    "SSE POST notification to '{}' for '{method}' failed",
                    self.url
                )
            })?;

        if !http_resp.status().is_success() {
            let status = http_resp.status();
            warn!(method = %method, %status, "SSE notification returned non-success");
        }

        Ok(())
    }

    async fn is_alive(&self) -> bool {
        // Try a lightweight HEAD request to check connectivity.
        self.client
            .head(&self.url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .is_ok()
    }

    async fn kill(&self) {
        // For SSE transport, there is no persistent connection to kill.
        // Clear any pending requests.
        let mut pending = self.pending.lock().await;
        pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sse_transport_creation() {
        let transport = SseTransport::new("http://localhost:8080/mcp");
        assert!(transport.is_ok());
    }

    #[test]
    fn test_sse_transport_invalid_url_still_creates() {
        // reqwest doesn't validate URLs at build time, only at request time
        let transport = SseTransport::new("not-a-url");
        assert!(transport.is_ok());
    }

    #[tokio::test]
    async fn test_sse_transport_is_alive_unreachable() {
        let transport = SseTransport::new("http://127.0.0.1:1/mcp").unwrap();
        assert!(!transport.is_alive().await);
    }

    #[tokio::test]
    async fn test_sse_transport_request_unreachable() {
        let transport = SseTransport::new("http://127.0.0.1:1/mcp").unwrap();
        let result = transport.request("test", None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_sse_transport_kill() {
        let transport = SseTransport::new("http://localhost:8080/mcp").unwrap();
        transport.kill().await;
        // Should not panic
    }
}
