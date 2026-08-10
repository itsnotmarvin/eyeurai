//! Shared HTTP plumbing for provider adapters.
//!
//! Responsibilities:
//!
//! * one `reqwest::Client` for the whole app, TLS-only, with connect and total
//!   deadlines so a hung endpoint can never wedge a refresh;
//! * `Authorization` headers built from [`Secret`] and marked *sensitive* so
//!   `reqwest`'s own `Debug` never prints them;
//! * a redirect policy that refuses HTTPS -> HTTP downgrades (a bearer token
//!   must never touch cleartext);
//! * bounded retry with `Retry-After` awareness for transient failures only;
//! * one status-classification table shared by every adapter, producing
//!   [`ProviderError`]s with the right [`crate::models::ProviderErrorKind`].
//!
//! The pure pieces ([`parse_retry_after`], [`classify_status`], [`decode_json`])
//! are separated from the I/O so they can be unit tested with fixtures.

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;

use super::error::{snippet, ProviderError};
use super::secret::Secret;

/// Hard ceiling on a response body we are willing to parse. Real quota
/// payloads are a few kilobytes; anything larger is a misconfigured proxy or an
/// HTML error page.
pub const MAX_BODY_BYTES: usize = 1 << 20; // 1 MiB

/// Default per-request deadline. Deliberately short: this runs on a menu-bar
/// refresh tick, so a slow provider must degrade rather than block the UI.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Default connect deadline.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Total budget for one provider (all accounts, all retries).
pub const DEFAULT_PROVIDER_BUDGET: Duration = Duration::from_secs(30);

/// Attempts (not retries): 1 means no retry.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// Largest delay we will honour from a `Retry-After` header.
pub const RETRY_AFTER_CAP: Duration = Duration::from_secs(8);

/// Tunables for [`HttpClient`].
#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub max_attempts: u32,
}

impl Default for HttpConfig {
    fn default() -> Self {
        HttpConfig {
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        }
    }
}

/// A single JSON GET, described declaratively so the adapter code stays flat.
pub struct JsonRequest<'a> {
    pub url: String,
    pub user_agent: &'a str,
    /// Bearer credential. `None` performs an unauthenticated GET.
    pub bearer: Option<&'a Secret>,
    /// Extra headers (never `Authorization` — use `bearer`).
    pub headers: Vec<(&'static str, String)>,
    pub timeout: Duration,
    pub max_attempts: u32,
}

impl<'a> JsonRequest<'a> {
    pub fn new(url: impl Into<String>, user_agent: &'a str) -> Self {
        JsonRequest {
            url: url.into(),
            user_agent,
            bearer: None,
            headers: Vec::new(),
            timeout: DEFAULT_REQUEST_TIMEOUT,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        }
    }

    pub fn bearer(mut self, secret: &'a Secret) -> Self {
        self.bearer = Some(secret);
        self
    }

    pub fn header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn max_attempts(mut self, attempts: u32) -> Self {
        self.max_attempts = attempts.max(1);
        self
    }
}

/// Thin wrapper over `reqwest::Client`.
#[derive(Clone, Debug)]
pub struct HttpClient {
    inner: reqwest::Client,
    config: HttpConfig,
}

impl HttpClient {
    pub fn new(config: HttpConfig) -> Result<Self, ProviderError> {
        let redirect = reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 3 {
                return attempt.error("too many redirects");
            }
            // Bearer tokens must never travel over cleartext.
            if attempt.url().scheme() != "https" {
                return attempt.error("refusing non-HTTPS redirect");
            }
            attempt.follow()
        });

        let inner = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .redirect(redirect)
            .https_only(true)
            .build()
            .map_err(|e| ProviderError::internal(format!("could not build HTTP client: {e}")))?;

        Ok(HttpClient { inner, config })
    }

    pub fn config(&self) -> &HttpConfig {
        &self.config
    }

    /// Perform a JSON GET with bounded retry.
    ///
    /// Retries happen only for [`ProviderError::is_retryable`] failures, and
    /// never more than `request.max_attempts` times in total. A `Retry-After`
    /// header is honoured, capped at [`RETRY_AFTER_CAP`].
    pub async fn get_json<T: DeserializeOwned>(
        &self,
        request: &JsonRequest<'_>,
    ) -> Result<T, ProviderError> {
        let attempts = request.max_attempts.max(1);
        let mut last: Option<ProviderError> = None;

        for attempt in 0..attempts {
            match self.get_json_once::<T>(request).await {
                Ok(value) => return Ok(value),
                Err(err) => {
                    let retryable = err.is_retryable();
                    let delay = err.retry_after.unwrap_or_else(|| backoff_delay(attempt));
                    last = Some(err);
                    if !retryable || attempt + 1 >= attempts {
                        break;
                    }
                    tokio::time::sleep(delay.min(RETRY_AFTER_CAP)).await;
                }
            }
        }

        Err(last.unwrap_or_else(|| ProviderError::internal("no attempt was made")))
    }

    async fn get_json_once<T: DeserializeOwned>(
        &self,
        request: &JsonRequest<'_>,
    ) -> Result<T, ProviderError> {
        let headers = build_headers(request)?;
        let response = self
            .inner
            .get(&request.url)
            .headers(headers)
            .timeout(request.timeout)
            .send()
            .await
            .map_err(|e| classify_transport_error(&e, &request.url))?;

        let status = response.status();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_retry_after);

        if let Some(len) = response.content_length() {
            if len as usize > MAX_BODY_BYTES {
                return Err(ProviderError::parse(format!(
                    "response from {} is {} bytes, above the {} byte cap",
                    host_of(&request.url),
                    len,
                    MAX_BODY_BYTES
                )));
            }
        }

        let body = response
            .text()
            .await
            .map_err(|e| classify_transport_error(&e, &request.url))?;

        if body.len() > MAX_BODY_BYTES {
            return Err(ProviderError::parse(format!(
                "response from {} exceeded the {} byte cap",
                host_of(&request.url),
                MAX_BODY_BYTES
            )));
        }

        if !status.is_success() {
            return Err(classify_status(
                status.as_u16(),
                retry_after,
                &body,
                &host_of(&request.url),
            ));
        }

        decode_json::<T>(&body, &host_of(&request.url))
    }
}

/// Build the header map for a request. `Authorization` is added last and marked
/// sensitive so `reqwest`/`http` never render it in `Debug` output.
fn build_headers(request: &JsonRequest<'_>) -> Result<HeaderMap, ProviderError> {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        USER_AGENT,
        HeaderValue::from_str(request.user_agent).map_err(|_| {
            ProviderError::internal("user agent contains characters invalid in a header")
        })?,
    );

    for (name, value) in &request.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| ProviderError::internal(format!("{name} is not a valid header name")))?;
        if name == AUTHORIZATION {
            // Programmer error: use JsonRequest::bearer.
            return Err(ProviderError::internal(
                "Authorization must be supplied through JsonRequest::bearer",
            ));
        }
        let value = HeaderValue::from_str(value).map_err(|_| {
            ProviderError::internal(format!("header {name} contains invalid characters"))
        })?;
        headers.insert(name, value);
    }

    if let Some(secret) = request.bearer {
        if secret.is_empty() {
            return Err(ProviderError::credentials_missing(
                "credential was found but is empty",
            ));
        }
        let mut value = HeaderValue::from_str(&format!("Bearer {}", secret.expose()))
            .map_err(|_| ProviderError::unauthorized("credential is not a valid header value"))?;
        value.set_sensitive(true);
        headers.insert(AUTHORIZATION, value);
    }

    Ok(headers)
}

/// Map a `reqwest` transport error onto our taxonomy.
///
/// The `reqwest::Error` `Display` can include the request URL; we pass it
/// through [`snippet`] (which scrubs) and only keep the host, so a URL that
/// carries an `?key=` parameter cannot leak into a message.
pub fn classify_transport_error(err: &reqwest::Error, url: &str) -> ProviderError {
    let host = host_of(url);
    if err.is_timeout() {
        return ProviderError::timeout(format!("{host} did not respond in time"))
            .with_remediation("Check your connection, then refresh.");
    }
    if err.is_connect() {
        return ProviderError::network(format!("could not connect to {host}"))
            .with_remediation("Check your network connection, then refresh.");
    }
    if err.is_decode() {
        return ProviderError::parse(format!("could not read the response from {host}"));
    }
    if err.is_body() || err.is_request() {
        return ProviderError::network(format!("request to {host} failed"));
    }
    ProviderError::network(format!("network error talking to {host}"))
}

/// Shared non-2xx classification.
///
/// * 401 -> `Unauthorized`
/// * 403 -> `Forbidden` (scope/entitlement, not a bad token)
/// * 404 -> `Unsupported` (endpoint gone or account lacks the surface)
/// * 408 / 425 / 429 / 5xx -> retryable (`RateLimited` for 429)
/// * everything else -> `Upstream`
pub fn classify_status(
    status: u16,
    retry_after: Option<Duration>,
    body: &str,
    host: &str,
) -> ProviderError {
    let detail = snippet(body);
    let suffix = if detail.is_empty() {
        String::new()
    } else {
        format!(": {detail}")
    };

    match status {
        401 => ProviderError::unauthorized(format!("{host} rejected the credential{suffix}")),
        403 => ProviderError::forbidden(format!(
            "{host} refused the request; the credential lacks the required access{suffix}"
        )),
        404 => ProviderError::unsupported(format!(
            "{host} has no usage endpoint for this account{suffix}"
        )),
        429 => ProviderError::rate_limited(format!("{host} is rate limiting requests{suffix}"))
            .with_retry_after(retry_after.or(Some(Duration::from_secs(5)))),
        408 | 425 => ProviderError::timeout(format!("{host} timed out the request{suffix}"))
            .with_retry_after(retry_after),
        s if (500..600).contains(&s) => {
            ProviderError::upstream(format!("{host} returned a server error (HTTP {s}){suffix}"))
                .with_retry_after(retry_after)
        }
        s => ProviderError::upstream(format!("{host} returned HTTP {s}{suffix}")),
    }
}

/// Decode a JSON body into `T`, turning a shape mismatch into a `Parse` error
/// that names the endpoint (so an upstream API change is diagnosable) without
/// echoing the whole payload.
pub fn decode_json<T: DeserializeOwned>(body: &str, host: &str) -> Result<T, ProviderError> {
    serde_json::from_str::<T>(body).map_err(|e| {
        ProviderError::parse(format!(
            "unexpected response shape from {host} ({e}); the upstream API may have changed"
        ))
        .with_remediation("Update EyeUrAI, or report this if it persists.")
    })
}

/// Parse a `Retry-After` header: delta-seconds or an HTTP date.
/// Returns `None` for anything unparseable, zero, or in the past.
pub fn parse_retry_after(raw: &str) -> Option<Duration> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(seconds) = raw.parse::<f64>() {
        if seconds <= 0.0 || !seconds.is_finite() {
            return None;
        }
        return Some(Duration::from_secs_f64(seconds.min(3600.0)));
    }
    // HTTP-date (IMF-fixdate is RFC 2822 compatible for chrono's parser).
    let when = chrono::DateTime::parse_from_rfc2822(raw).ok()?;
    let delta = when.with_timezone(&chrono::Utc) - chrono::Utc::now();
    let seconds = delta.num_seconds();
    if seconds <= 0 {
        return None;
    }
    Some(Duration::from_secs(seconds.min(3600) as u64))
}

/// Exponential backoff with a fixed schedule (400ms, 800ms, 1600ms ...),
/// capped. Deterministic — no RNG dependency.
pub fn backoff_delay(attempt: u32) -> Duration {
    let millis = 400u64.saturating_mul(1u64 << attempt.min(4));
    Duration::from_millis(millis.min(3_200))
}

/// Extract just the host from a URL for use in user-facing messages. Falls back
/// to a scrubbed form of the input if the URL cannot be parsed, so query
/// parameters can never leak.
pub fn host_of(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(parsed) => parsed.host_str().unwrap_or("the provider").to_string(),
        Err(_) => "the provider".to_string(),
    }
}

/// Reject anything that is not a conservative identifier before it is placed
/// into a URL path segment. Used for user-supplied values such as a Google
/// Cloud project id.
pub fn validate_path_segment(value: &str, what: &str) -> Result<(), ProviderError> {
    let ok = !value.is_empty()
        && value.len() <= 128
        && !value.contains("..")
        && value != "."
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == ':');
    if ok {
        Ok(())
    } else {
        Err(ProviderError::unsupported(format!(
            "{what} contains characters that are not allowed"
        )))
    }
}

/// Status codes that should be surfaced verbatim by adapters wanting to
/// override the default table (e.g. Codex's revoked-token 401 body).
pub fn is_status(status: &StatusCode, code: u16) -> bool {
    status.as_u16() == code
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ProviderErrorKind;
    use serde::Deserialize;

    #[test]
    fn retry_after_accepts_delta_seconds() {
        assert_eq!(parse_retry_after("5"), Some(Duration::from_secs(5)));
        assert_eq!(parse_retry_after("  12  "), Some(Duration::from_secs(12)));
        assert_eq!(parse_retry_after("0"), None);
        assert_eq!(parse_retry_after("-3"), None);
        assert_eq!(parse_retry_after(""), None);
        assert_eq!(parse_retry_after("soon"), None);
    }

    #[test]
    fn retry_after_is_capped_at_one_hour() {
        assert_eq!(parse_retry_after("99999"), Some(Duration::from_secs(3600)));
    }

    #[test]
    fn retry_after_rejects_past_http_dates() {
        assert_eq!(parse_retry_after("Sun, 06 Nov 1994 08:49:37 +0000"), None);
    }

    #[test]
    fn status_classification_covers_the_taxonomy() {
        let cases: [(u16, ProviderErrorKind); 8] = [
            (401, ProviderErrorKind::Unauthorized),
            (403, ProviderErrorKind::Forbidden),
            (404, ProviderErrorKind::Unsupported),
            (408, ProviderErrorKind::Timeout),
            (429, ProviderErrorKind::RateLimited),
            (500, ProviderErrorKind::Upstream),
            (503, ProviderErrorKind::Upstream),
            (418, ProviderErrorKind::Upstream),
        ];
        for (status, expected) in cases {
            let err = classify_status(status, None, "", "example.test");
            assert_eq!(err.kind, expected, "status {status}");
        }
    }

    #[test]
    fn rate_limited_gets_a_default_retry_after() {
        let err = classify_status(429, None, "slow down", "example.test");
        assert_eq!(err.retry_after, Some(Duration::from_secs(5)));
        let err = classify_status(429, Some(Duration::from_secs(30)), "", "example.test");
        assert_eq!(err.retry_after, Some(Duration::from_secs(30)));
    }

    #[test]
    fn status_messages_scrub_echoed_credentials() {
        let body = r#"{"error":{"message":"invalid api key sk-ant-oat01-abcdefghij"}}"#;
        let err = classify_status(401, None, body, "api.anthropic.com");
        assert!(!err.message.contains("abcdefghij"));
        assert!(err.message.contains("api.anthropic.com"));
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Sample {
        value: u32,
    }

    #[test]
    fn decode_json_reports_shape_drift_as_parse_error() {
        let ok: Sample = decode_json(r#"{"value":7}"#, "example.test").expect("decodes");
        assert_eq!(ok, Sample { value: 7 });

        let err = decode_json::<Sample>(r#"{"value":"seven"}"#, "example.test")
            .expect_err("should not decode");
        assert_eq!(err.kind, ProviderErrorKind::Parse);
        assert!(err.message.contains("example.test"));
        assert!(err.remediation.is_some());
    }

    #[test]
    fn host_of_never_leaks_query_parameters() {
        assert_eq!(
            host_of("https://generativelanguage.googleapis.com/v1/models?key=AIzaSecret"),
            "generativelanguage.googleapis.com"
        );
        assert_eq!(host_of("not a url"), "the provider");
    }

    #[test]
    fn backoff_is_bounded_and_monotonic() {
        assert_eq!(backoff_delay(0), Duration::from_millis(400));
        assert_eq!(backoff_delay(1), Duration::from_millis(800));
        assert_eq!(backoff_delay(2), Duration::from_millis(1_600));
        assert_eq!(backoff_delay(9), Duration::from_millis(3_200));
    }

    #[test]
    fn path_segment_validation_rejects_traversal_and_separators() {
        assert!(validate_path_segment("my-project-123", "project id").is_ok());
        assert!(validate_path_segment("google.com:my-project", "project id").is_ok());
        assert!(validate_path_segment("../etc/passwd", "project id").is_err());
        assert!(validate_path_segment("..", "project id").is_err());
        assert!(validate_path_segment(".", "project id").is_err());
        assert!(validate_path_segment("a b", "project id").is_err());
        assert!(validate_path_segment("", "project id").is_err());
        assert!(validate_path_segment(&"x".repeat(200), "project id").is_err());
    }
}
