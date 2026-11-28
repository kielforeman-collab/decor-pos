//! HTTP security hardening middleware shared by `df-edge` and `df-api`
//! (closes security-review finding F10: CORS allowlist, auth rate
//! limiting, request body limits, and defensive response headers).
//!
//! No external rate-limit/CORS crates are required: everything runs over
//! `axum` + `std` so offline builds stay dependency-free.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    ACCESS_CONTROL_MAX_AGE, ACCESS_CONTROL_REQUEST_METHOD, CACHE_CONTROL, HeaderName, HeaderValue,
    ORIGIN, REFERRER_POLICY, RETRY_AFTER, VARY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use axum::http::{Method, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};

use crate::ErrorBody;

/// Default maximum HTTP request body size (4 MiB — accommodates large
/// product-import batches and 1,000-entry sync batches while blocking
/// unbounded memory DoS).
pub const DEFAULT_MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

/// Default per-client auth attempt cap within one rate-limit window.
pub const DEFAULT_AUTH_RATE_LIMIT_MAX: u32 = 20;

/// Default auth rate-limit window length in seconds.
pub const DEFAULT_AUTH_RATE_LIMIT_WINDOW_SECS: u64 = 60;

const MAX_RATE_LIMIT_BUCKETS: usize = 1024;

/// Configuration for the per-client auth endpoint rate limiter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthRateLimitConfig {
    /// Maximum requests per client per window (`0` disables rate limiting).
    pub max_requests: u32,
    /// Fixed window duration.
    pub window: Duration,
    /// When `true`, extract the client IP from the first entry in
    /// `X-Forwarded-For` before falling back to `ConnectInfo<SocketAddr>`.
    /// Keep `false` unless the server sits strictly behind a trusted
    /// reverse proxy that strips/overwrites `X-Forwarded-For`.
    pub trust_proxy_headers: bool,
}

impl Default for AuthRateLimitConfig {
    fn default() -> Self {
        Self {
            max_requests: DEFAULT_AUTH_RATE_LIMIT_MAX,
            window: Duration::from_secs(DEFAULT_AUTH_RATE_LIMIT_WINDOW_SECS),
            trust_proxy_headers: false,
        }
    }
}

/// Shared HTTP hardening settings applied to `df-edge` and `df-api`
/// routers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpHardeningConfig {
    /// Exact allowed browser origins (e.g. `http://localhost:1420`).
    /// Empty (default) emits no `Access-Control-Allow-Origin` header,
    /// blocking cross-origin browser calls fail-closed. Wildcard `*` is
    /// rejected by [`HttpHardeningConfig::validate`].
    pub cors_origins: Vec<String>,
    /// Rate limit applied to credential-checking auth endpoints
    /// (`/login`, `/portal-login`, `/counter-switch`, `/override-verify`,
    /// `/setup`) to protect Argon2id CPU and thwart credential stuffing.
    pub auth_rate_limit: AuthRateLimitConfig,
    /// Maximum request body size in bytes (`DefaultBodyLimit`).
    pub max_body_bytes: usize,
}

impl Default for HttpHardeningConfig {
    fn default() -> Self {
        Self {
            cors_origins: Vec::new(),
            auth_rate_limit: AuthRateLimitConfig::default(),
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
        }
    }
}

impl HttpHardeningConfig {
    /// Build hardening config from environment variables:
    ///
    /// * `DF_CORS_ORIGINS` — comma-separated exact origins.
    /// * `DF_AUTH_RATE_LIMIT_MAX` — max auth attempts per window (`0` disables).
    /// * `DF_AUTH_RATE_LIMIT_WINDOW_SECS` — window duration in seconds.
    /// * `DF_MAX_BODY_BYTES` — max request body size in bytes.
    /// * `DF_TRUST_PROXY_HEADERS` — `1`/`true`/`yes` to trust `X-Forwarded-For`.
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Ok(raw) = std::env::var("DF_CORS_ORIGINS") {
            cfg.cors_origins = raw
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
        }
        if let Ok(raw) = std::env::var("DF_AUTH_RATE_LIMIT_MAX")
            && let Ok(max) = raw.trim().parse::<u32>()
        {
            cfg.auth_rate_limit.max_requests = max;
        }
        if let Ok(raw) = std::env::var("DF_AUTH_RATE_LIMIT_WINDOW_SECS")
            && let Ok(secs) = raw.trim().parse::<u64>()
            && secs > 0
        {
            cfg.auth_rate_limit.window = Duration::from_secs(secs);
        }
        if let Ok(raw) = std::env::var("DF_MAX_BODY_BYTES")
            && let Ok(bytes) = raw.trim().parse::<usize>()
            && bytes > 0
        {
            cfg.max_body_bytes = bytes;
        }
        if let Ok(raw) = std::env::var("DF_TRUST_PROXY_HEADERS") {
            cfg.auth_rate_limit.trust_proxy_headers = matches!(
                raw.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            );
        }
        cfg
    }

    /// Reject wildcard `*` or malformed origins up front at server startup.
    pub fn validate(&self) -> Result<(), String> {
        for origin in &self.cors_origins {
            let trimmed = origin.trim();
            if trimmed == "*" {
                return Err(
                    "wildcard '*' is not allowed in CORS origins; list exact origins instead"
                        .to_string(),
                );
            }
            if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
                return Err(format!(
                    "invalid CORS origin {origin:?}: must start with http:// or https://"
                ));
            }
            if HeaderValue::from_str(trimmed).is_err() {
                return Err(format!("invalid CORS origin header value {origin:?}"));
            }
        }
        Ok(())
    }
}

// --- auth rate limiter ----------------------------------------------------

#[derive(Debug, Clone)]
struct Bucket {
    count: u32,
    window_start: Instant,
}

/// Shared, thread-safe fixed-window rate limiter keyed by client IP.
#[derive(Debug, Clone)]
pub struct AuthRateLimiter {
    config: AuthRateLimitConfig,
    buckets: Arc<Mutex<HashMap<String, Bucket>>>,
}

impl AuthRateLimiter {
    pub fn new(config: AuthRateLimitConfig) -> Self {
        Self {
            config,
            buckets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check and record one request for `client_key`. Returns `Ok(())`
    /// when allowed, or `Err(retry_after_secs)` when the limit is
    /// exceeded.
    pub fn check(&self, client_key: &str) -> Result<(), u64> {
        if self.config.max_requests == 0 {
            return Ok(());
        }
        let now = Instant::now();
        let window = self.config.window;
        let mut map = self.buckets.lock().expect("auth rate limiter lock");
        if map.len() > MAX_RATE_LIMIT_BUCKETS {
            map.retain(|_, b| now.duration_since(b.window_start) < window);
        }
        let entry = map.entry(client_key.to_string()).or_insert(Bucket {
            count: 0,
            window_start: now,
        });
        let elapsed = now.duration_since(entry.window_start);
        if elapsed >= window {
            entry.count = 1;
            entry.window_start = now;
            return Ok(());
        }
        if entry.count >= self.config.max_requests {
            let remaining = window.saturating_sub(elapsed).as_secs().max(1);
            return Err(remaining);
        }
        entry.count += 1;
        Ok(())
    }

    fn client_key(&self, req: &Request<Body>) -> String {
        if self.config.trust_proxy_headers
            && let Some(xff) = req
                .headers()
                .get(HeaderName::from_static("x-forwarded-for"))
                .and_then(|v| v.to_str().ok())
            && let Some(first) = xff
                .split(',')
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        {
            return first.to_string();
        }
        if let Some(ConnectInfo(addr)) = req.extensions().get::<ConnectInfo<SocketAddr>>() {
            return addr.ip().to_string();
        }
        "local".to_string()
    }
}

/// Axum middleware that enforces [`AuthRateLimiter`] on auth routes,
/// returning `429 Too Many Requests` + `Retry-After` before Argon2id runs.
pub async fn auth_rate_limit(
    State(limiter): State<AuthRateLimiter>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let key = limiter.client_key(&req);
    if let Err(retry_after) = limiter.check(&key) {
        let mut res = (
            StatusCode::TOO_MANY_REQUESTS,
            Json(ErrorBody {
                error: "too many authentication attempts — try again later".to_string(),
            }),
        )
            .into_response();
        if let Ok(val) = HeaderValue::from_str(&retry_after.to_string()) {
            res.headers_mut().insert(RETRY_AFTER, val);
        }
        return res;
    }
    next.run(req).await
}

// --- strict exact-origin CORS middleware ----------------------------------

#[derive(Debug, Clone)]
struct CorsAllowlist {
    origins: Arc<HashSet<String>>,
}

impl CorsAllowlist {
    fn new(origins: &[String]) -> Self {
        let set: HashSet<String> = origins
            .iter()
            .map(|o| o.trim().trim_end_matches('/').to_string())
            .filter(|o| !o.is_empty() && o != "*")
            .collect();
        Self {
            origins: Arc::new(set),
        }
    }

    fn matches(&self, origin: &str) -> bool {
        let normalized = origin.trim().trim_end_matches('/');
        self.origins.contains(normalized)
    }
}

async fn cors_middleware(
    State(allowlist): State<CorsAllowlist>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let origin = req
        .headers()
        .get(ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let is_preflight = req.method() == Method::OPTIONS
        && req.headers().contains_key(ACCESS_CONTROL_REQUEST_METHOD);

    let allowed_origin = origin
        .as_deref()
        .filter(|o| allowlist.matches(o))
        .and_then(|o| HeaderValue::from_str(o).ok());

    if is_preflight {
        let mut res = StatusCode::NO_CONTENT.into_response();
        if let Some(origin_val) = allowed_origin {
            let headers = res.headers_mut();
            headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, origin_val);
            headers.insert(
                ACCESS_CONTROL_ALLOW_METHODS,
                HeaderValue::from_static("GET, POST, PUT, PATCH, DELETE, OPTIONS"),
            );
            headers.insert(
                ACCESS_CONTROL_ALLOW_HEADERS,
                HeaderValue::from_static("Authorization, Content-Type"),
            );
            headers.insert(ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
            headers.insert(VARY, HeaderValue::from_static("Origin"));
        } else if !allowlist.origins.is_empty() {
            res.headers_mut()
                .insert(VARY, HeaderValue::from_static("Origin"));
        }
        return res;
    }

    let mut res = next.run(req).await;
    if let Some(origin_val) = allowed_origin {
        res.headers_mut()
            .insert(ACCESS_CONTROL_ALLOW_ORIGIN, origin_val);
        res.headers_mut()
            .insert(VARY, HeaderValue::from_static("Origin"));
    } else if !allowlist.origins.is_empty() {
        res.headers_mut()
            .insert(VARY, HeaderValue::from_static("Origin"));
    }
    res
}

// --- defensive security response headers ----------------------------------

async fn security_headers(req: Request<Body>, next: Next) -> Response {
    let is_auth_path = req.uri().path().contains("/auth/");
    let mut res = next.run(req).await;
    let headers = res.headers_mut();
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    if is_auth_path {
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
}

/// Apply body size limit, security response headers, and exact-origin
/// CORS enforcement to any router.
pub fn apply_http_hardening(router: Router, config: &HttpHardeningConfig) -> Router {
    let allowlist = CorsAllowlist::new(&config.cors_origins);
    router
        .layer(DefaultBodyLimit::max(config.max_body_bytes))
        .layer(middleware::from_fn(security_headers))
        .layer(middleware::from_fn_with_state(allowlist, cors_middleware))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use axum::routing::{get, post};
    use tower::ServiceExt;

    #[test]
    fn validate_rejects_wildcard_and_bad_schemes() {
        let mut cfg = HttpHardeningConfig::default();
        assert!(cfg.validate().is_ok());
        cfg.cors_origins = vec!["https://pos.example.com".to_string()];
        assert!(cfg.validate().is_ok());

        cfg.cors_origins = vec!["*".to_string()];
        assert!(cfg.validate().is_err());

        cfg.cors_origins = vec!["ftp://bad.example.com".to_string()];
        assert!(cfg.validate().is_err());
    }

    #[tokio::test]
    async fn cors_allows_only_configured_origins_and_sets_security_headers() {
        let cfg = HttpHardeningConfig {
            cors_origins: vec!["https://pos.example.com".to_string()],
            ..HttpHardeningConfig::default()
        };
        let app = apply_http_hardening(
            Router::new().route("/api/v1/auth/login", post(|| async { "ok" })),
            &cfg,
        );

        // Allowed preflight
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/api/v1/auth/login")
                    .header(ORIGIN, "https://pos.example.com")
                    .header(ACCESS_CONTROL_REQUEST_METHOD, "POST")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            res.headers()
                .get(ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|v| v.to_str().ok()),
            Some("https://pos.example.com")
        );

        // Unlisted origin gets no Allow-Origin header
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/auth/login")
                    .header(ORIGIN, "https://evil.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(res.headers().get(ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
        assert_eq!(
            res.headers()
                .get(X_CONTENT_TYPE_OPTIONS)
                .and_then(|v| v.to_str().ok()),
            Some("nosniff")
        );
        assert_eq!(
            res.headers()
                .get(CACHE_CONTROL)
                .and_then(|v| v.to_str().ok()),
            Some("no-store")
        );
    }

    #[tokio::test]
    async fn rate_limiter_blocks_after_max_requests() {
        let limiter = AuthRateLimiter::new(AuthRateLimitConfig {
            max_requests: 2,
            window: Duration::from_secs(60),
            trust_proxy_headers: true,
        });
        let app = Router::new()
            .route("/login", post(|| async { "ok" }))
            .route_layer(middleware::from_fn_with_state(limiter, auth_rate_limit));

        for expected in [
            StatusCode::OK,
            StatusCode::OK,
            StatusCode::TOO_MANY_REQUESTS,
        ] {
            let res = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/login")
                        .header("x-forwarded-for", "203.0.113.10")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(res.status(), expected);
            if expected == StatusCode::TOO_MANY_REQUESTS {
                assert!(res.headers().get(RETRY_AFTER).is_some());
            }
        }

        // Different IP is still allowed.
        let res = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/login")
                    .header("x-forwarded-for", "203.0.113.11")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn body_limit_rejects_oversized_payloads() {
        let cfg = HttpHardeningConfig {
            max_body_bytes: 32,
            ..HttpHardeningConfig::default()
        };
        let app = apply_http_hardening(
            Router::new().route("/upload", post(|_body: String| async { "ok" })),
            &cfg,
        );
        let res = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/upload")
                    .body(Body::from("x".repeat(64)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);

        // Health routes still get security headers without Cache-Control: no-store.
        let app2 =
            apply_http_hardening(Router::new().route("/health", get(|| async { "ok" })), &cfg);
        let res2 = app2
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res2.status(), StatusCode::OK);
        assert!(res2.headers().get(CACHE_CONTROL).is_none());
        assert_eq!(
            res2.headers()
                .get(X_FRAME_OPTIONS)
                .and_then(|v| v.to_str().ok()),
            Some("DENY")
        );
    }
}
