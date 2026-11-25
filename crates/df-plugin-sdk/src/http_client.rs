//! Minimal HTTP/1.1 client over `tokio::TcpStream`, for talking to
//! local plugin processes.
//!
//! Deliberately dependency-free (no `reqwest` — it is not in the
//! offline crate cache): plain `http://` only, loopback hosts only
//! (`127.0.0.1`, `localhost`, `::1`), `Connection: close`, with
//! `Content-Length` and chunked response support. This is a real HTTP
//! client, not a mock — the tests exercise it against a real axum
//! server. HTTPS, proxies, redirects and cookies are out of scope:
//! plugins are local child processes.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{PluginError, Result};

/// An HTTP response: status code plus raw body bytes.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// Body as UTF-8 text.
    pub fn body_str(&self) -> Result<&str> {
        std::str::from_utf8(&self.body)
            .map_err(|e| PluginError::InvalidResponse(format!("response body is not UTF-8: {e}")))
    }

    /// Body parsed as JSON.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.body).map_err(|e| {
            PluginError::InvalidResponse(format!("response body is not valid JSON: {e}"))
        })
    }

    /// Require a 2xx status, else [`PluginError::HttpStatus`].
    pub fn require_ok(&self, path: &str) -> Result<()> {
        if (200..300).contains(&self.status) {
            Ok(())
        } else {
            Err(PluginError::HttpStatus {
                status: self.status,
                path: path.to_string(),
                body: String::from_utf8_lossy(&self.body).into_owned(),
            })
        }
    }
}

struct ParsedUrl {
    host: String,
    port: u16,
    path: String,
}

fn parse_url(url: &str) -> Result<ParsedUrl> {
    let rest = url.strip_prefix("http://").ok_or_else(|| {
        PluginError::Http(format!(
            "only plain http:// plugin URLs are supported, got {url:?}"
        ))
    })?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], rest[index..].to_string()),
        None => (rest, "/".to_string()),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() => {
            let port: u16 = port
                .parse()
                .map_err(|_| PluginError::Http(format!("invalid port in plugin URL {url:?}")))?;
            (host.to_string(), port)
        }
        _ => (authority.to_string(), 80),
    };
    let host_bare = host.trim_matches(|c| c == '[' || c == ']');
    if host_bare != "127.0.0.1" && host_bare != "localhost" && host_bare != "::1" {
        return Err(PluginError::Http(format!(
            "plugin endpoints must be loopback (127.0.0.1/localhost), got host {host:?}"
        )));
    }
    Ok(ParsedUrl { host, port, path })
}

fn decode_chunked(body: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut rest = body;
    loop {
        let line_end = rest
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| PluginError::InvalidResponse("truncated chunked body".to_string()))?;
        let size_line = std::str::from_utf8(&rest[..line_end]).map_err(|_| {
            PluginError::InvalidResponse("chunk size line is not UTF-8".to_string())
        })?;
        let size_str = size_line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_str, 16).map_err(|_| {
            PluginError::InvalidResponse(format!("invalid chunk size {size_str:?}"))
        })?;
        rest = &rest[line_end + 2..];
        if size == 0 {
            break;
        }
        if rest.len() < size + 2 {
            return Err(PluginError::InvalidResponse(
                "truncated chunked body".to_string(),
            ));
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
    Ok(out)
}

async fn request(
    method: &str,
    url: &str,
    json_body: Option<&serde_json::Value>,
    timeout: Duration,
) -> Result<HttpResponse> {
    let parsed = parse_url(url)?;
    let body_bytes = json_body.map(|v| v.to_string()).unwrap_or_default();
    let mut request = format!(
        "{method} {} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n",
        parsed.path, parsed.host, parsed.port
    );
    if json_body.is_some() {
        request.push_str("Content-Type: application/json\r\n");
        request.push_str(&format!("Content-Length: {}\r\n", body_bytes.len()));
    }
    request.push_str("\r\n");
    request.push_str(&body_bytes);

    let connect = tokio::net::TcpStream::connect((parsed.host.as_str(), parsed.port));
    let mut stream = tokio::time::timeout(timeout, connect)
        .await
        .map_err(|_| PluginError::Http(format!("connect to {url} timed out")))?
        .map_err(|e| PluginError::Http(format!("could not connect to {url}: {e}")))?;
    tokio::time::timeout(timeout, stream.write_all(request.as_bytes()))
        .await
        .map_err(|_| PluginError::Http("write to plugin timed out".to_string()))?
        .map_err(|e| PluginError::Http(format!("could not write to plugin: {e}")))?;

    let mut raw = Vec::new();
    tokio::time::timeout(timeout, stream.read_to_end(&mut raw))
        .await
        .map_err(|_| PluginError::Http("read from plugin timed out".to_string()))?
        .map_err(|e| PluginError::Http(format!("could not read from plugin: {e}")))?;

    let header_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| {
            PluginError::InvalidResponse("plugin response has no header terminator".to_string())
        })?;
    let header_text = std::str::from_utf8(&raw[..header_end]).map_err(|_| {
        PluginError::InvalidResponse("plugin response headers are not UTF-8".to_string())
    })?;
    let mut lines = header_text.lines();
    let status_line = lines
        .next()
        .ok_or_else(|| PluginError::InvalidResponse("plugin response is empty".to_string()))?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| PluginError::InvalidResponse(format!("bad status line {status_line:?}")))?;
    let mut chunked = false;
    let mut content_length: Option<usize> = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            match name.trim().to_ascii_lowercase().as_str() {
                "transfer-encoding" if value.trim().eq_ignore_ascii_case("chunked") => {
                    chunked = true;
                }
                "content-length" => {
                    content_length = value.trim().parse().ok();
                }
                _ => {}
            }
        }
    }
    let body_raw = &raw[header_end + 4..];
    let body = if chunked {
        decode_chunked(body_raw)?
    } else if let Some(len) = content_length {
        if body_raw.len() < len {
            return Err(PluginError::InvalidResponse(format!(
                "response body truncated: got {} bytes, Content-Length {len}",
                body_raw.len()
            )));
        }
        body_raw[..len].to_vec()
    } else {
        body_raw.to_vec()
    };
    Ok(HttpResponse { status, body })
}

/// `GET` with the default 10 s timeout.
pub async fn http_get(url: &str) -> Result<HttpResponse> {
    http_get_timeout(url, Duration::from_secs(10)).await
}

/// `GET` with an explicit timeout.
pub async fn http_get_timeout(url: &str, timeout: Duration) -> Result<HttpResponse> {
    request("GET", url, None, timeout).await
}

/// `POST` with a JSON body, with the default 10 s timeout.
pub async fn http_post_json(url: &str, body: &serde_json::Value) -> Result<HttpResponse> {
    http_post_json_timeout(url, body, Duration::from_secs(10)).await
}

/// `POST` with a JSON body and an explicit timeout.
pub async fn http_post_json_timeout(
    url: &str,
    body: &serde_json::Value,
    timeout: Duration,
) -> Result<HttpResponse> {
    request("POST", url, Some(body), timeout).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Json;
    use axum::routing::{get, post};

    async fn test_server() -> (String, tokio::task::JoinHandle<()>) {
        let app = axum::Router::new()
            .route(
                "/health",
                get(|| async { Json(serde_json::json!({"status": "ok"})) }),
            )
            .route(
                "/echo",
                post(|Json(body): Json<serde_json::Value>| async move { Json(body) }),
            )
            .route(
                "/fail",
                get(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom") }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://127.0.0.1:{}", addr.port()), handle)
    }

    #[tokio::test]
    async fn get_and_post_against_a_real_server() {
        let (base, _handle) = test_server().await;
        let health = http_get(&format!("{base}/health")).await.unwrap();
        assert_eq!(health.status, 200);
        assert_eq!(health.json::<serde_json::Value>().unwrap()["status"], "ok");
        let echo = http_post_json(&format!("{base}/echo"), &serde_json::json!({"a": 1}))
            .await
            .unwrap();
        assert_eq!(echo.json::<serde_json::Value>().unwrap()["a"], 1);
        let fail = http_get(&format!("{base}/fail")).await.unwrap();
        assert_eq!(fail.status, 500);
        assert!(fail.require_ok("/fail").is_err());
    }

    #[tokio::test]
    async fn non_loopback_urls_are_refused() {
        let err = http_get("http://example.invalid/health").await.unwrap_err();
        assert!(matches!(err, PluginError::Http(_)));
        assert!(http_get("https://127.0.0.1:1/health").await.is_err());
    }
}
