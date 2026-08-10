//! Actionable provider errors.
//!
//! Every failure path in a provider adapter must land in exactly one
//! [`ProviderErrorKind`] so the UI can decide between "retry in a moment",
//! "sign in again", "this is not supported", and "we have a bug".
//!
//! `ProviderError` is the internal `Result` type; [`crate::models::ProviderErrorInfo`]
//! is its serializable projection. Messages are scrubbed of token-shaped
//! substrings by [`scrub`] before they are ever attached.

use std::fmt;
use std::time::Duration;

use crate::models::{ProviderErrorInfo, ProviderErrorKind};

/// Internal error type for provider adapters.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderError {
    pub kind: ProviderErrorKind,
    /// Already scrubbed; safe to display and to log.
    pub message: String,
    pub retry_after: Option<Duration>,
    pub remediation: Option<String>,
    pub doc_url: Option<String>,
}

impl ProviderError {
    pub fn new(kind: ProviderErrorKind, message: impl Into<String>) -> Self {
        ProviderError {
            kind,
            message: scrub(&message.into()),
            retry_after: None,
            remediation: None,
            doc_url: None,
        }
    }

    pub fn credentials_missing(message: impl Into<String>) -> Self {
        ProviderError::new(ProviderErrorKind::CredentialsMissing, message)
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        ProviderError::new(ProviderErrorKind::Unauthorized, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        ProviderError::new(ProviderErrorKind::Forbidden, message)
    }

    pub fn rate_limited(message: impl Into<String>) -> Self {
        ProviderError::new(ProviderErrorKind::RateLimited, message)
    }

    pub fn network(message: impl Into<String>) -> Self {
        ProviderError::new(ProviderErrorKind::Network, message)
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        ProviderError::new(ProviderErrorKind::Timeout, message)
    }

    pub fn parse(message: impl Into<String>) -> Self {
        ProviderError::new(ProviderErrorKind::Parse, message)
    }

    pub fn upstream(message: impl Into<String>) -> Self {
        ProviderError::new(ProviderErrorKind::Upstream, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        ProviderError::new(ProviderErrorKind::Unsupported, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        ProviderError::new(ProviderErrorKind::Internal, message)
    }

    pub fn with_remediation(mut self, remediation: impl Into<String>) -> Self {
        self.remediation = Some(remediation.into());
        self
    }

    pub fn with_retry_after(mut self, retry_after: Option<Duration>) -> Self {
        self.retry_after = retry_after;
        self
    }

    pub fn with_doc_url(mut self, url: impl Into<String>) -> Self {
        self.doc_url = Some(url.into());
        self
    }

    /// Whether an automatic retry within the same refresh is worthwhile.
    pub fn is_retryable(&self) -> bool {
        self.kind.is_retryable()
    }

    /// Serializable projection for the snapshot.
    pub fn to_info(&self) -> ProviderErrorInfo {
        ProviderErrorInfo {
            kind: self.kind,
            message: self.message.clone(),
            retryable: self.is_retryable(),
            retry_after_seconds: self.retry_after.map(|d| d.as_secs()),
            remediation: self.remediation.clone(),
            doc_url: self.doc_url.clone(),
        }
    }
}

impl From<ProviderError> for ProviderErrorInfo {
    fn from(value: ProviderError) -> Self {
        value.to_info()
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ProviderError {}

/// Maximum number of body bytes we ever quote back in an error message.
const SNIPPET_LIMIT: usize = 180;

/// Truncate an upstream response body for inclusion in an error message.
/// Also collapses newlines so a single log line stays a single line.
pub fn snippet(body: &str) -> String {
    let flat: String = body
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = flat.trim();
    let mut out: String = trimmed.chars().take(SNIPPET_LIMIT).collect();
    if trimmed.chars().count() > SNIPPET_LIMIT {
        out.push('…');
    }
    scrub(&out)
}

/// Redact token-shaped substrings from arbitrary text.
///
/// This is defence in depth: provider adapters never interpolate a [`crate::providers::secret::Secret`]
/// into a message, but an upstream error body could echo a key back, and a
/// URL could carry an `?key=` query parameter. Handled shapes:
///
/// * `sk-…`, `sk-or-…`, `sk-ant-…`, `AIza…`, `ya29.…` prefixed tokens
/// * three-segment JWTs beginning `eyJ`
/// * `key=`, `api_key=`, `access_token=`, `token=` query parameters
/// * `Bearer <value>`
pub fn scrub(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes: Vec<char> = input.chars().collect();
    let mut i = 0usize;

    while i < bytes.len() {
        if let Some(consumed) = match_query_secret(&bytes, i) {
            out.push_str(&consumed.0);
            out.push_str("[redacted]");
            i += consumed.1;
            continue;
        }
        if let Some(consumed) = match_bearer(&bytes, i) {
            out.push_str("Bearer [redacted]");
            i += consumed;
            continue;
        }
        if is_token_boundary(&bytes, i) {
            if let Some(consumed) = match_token_prefix(&bytes, i) {
                out.push_str("[redacted]");
                i += consumed;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// A token can only start at the beginning of the string or after a character
/// that cannot be part of a token (so `basketball` never matches `sk-`).
fn is_token_boundary(chars: &[char], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let prev = chars[i - 1];
    !(prev.is_ascii_alphanumeric() || prev == '-' || prev == '_' || prev == '.')
}

const TOKEN_PREFIXES: [&str; 4] = ["sk-", "AIza", "ya29.", "eyJ"];

/// Returns the number of characters consumed if a token-shaped run starts at `i`.
fn match_token_prefix(chars: &[char], i: usize) -> Option<usize> {
    let prefix = TOKEN_PREFIXES
        .iter()
        .find(|p| starts_with(chars, i, p))
        .copied()?;

    let mut j = i + prefix.chars().count();
    while j < chars.len() && is_token_char(chars[j]) {
        j += 1;
    }
    // Require a meaningful amount of material after the prefix so that plain
    // prose like "sk-" alone is not mangled.
    if j - i < prefix.chars().count() + 6 {
        return None;
    }
    Some(j - i)
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'
}

const SECRET_QUERY_KEYS: [&str; 6] = [
    "key=",
    "api_key=",
    "apikey=",
    "access_token=",
    "refresh_token=",
    "token=",
];

/// Matches `<key>=<value>` and returns `(literal key including '=', consumed)`.
fn match_query_secret(chars: &[char], i: usize) -> Option<(String, usize)> {
    if i > 0 {
        let prev = chars[i - 1];
        // Only match at a query/parameter boundary.
        if !(prev == '?' || prev == '&' || prev == ' ' || prev == '"' || prev == ',') {
            return None;
        }
    }
    let key = SECRET_QUERY_KEYS
        .iter()
        .find(|k| starts_with_ignore_case(chars, i, k))
        .copied()?;
    let key_len = key.chars().count();
    let mut j = i + key_len;
    let mut value_len = 0usize;
    while j < chars.len() && !matches!(chars[j], '&' | ' ' | '"' | '\'' | ',' | '}' | ')') {
        j += 1;
        value_len += 1;
    }
    if value_len == 0 {
        return None;
    }
    let literal: String = chars[i..i + key_len].iter().collect();
    Some((literal, key_len + value_len))
}

fn match_bearer(chars: &[char], i: usize) -> Option<usize> {
    if !starts_with_ignore_case(chars, i, "bearer ") {
        return None;
    }
    if i > 0 && chars[i - 1].is_ascii_alphanumeric() {
        return None;
    }
    let mut j = i + "bearer ".chars().count();
    let mut value_len = 0usize;
    while j < chars.len() && !chars[j].is_whitespace() && chars[j] != '"' {
        j += 1;
        value_len += 1;
    }
    if value_len == 0 {
        return None;
    }
    Some("bearer ".chars().count() + value_len)
}

fn starts_with(chars: &[char], i: usize, needle: &str) -> bool {
    for (idx, c) in (i..).zip(needle.chars()) {
        if idx >= chars.len() || chars[idx] != c {
            return false;
        }
    }
    true
}

fn starts_with_ignore_case(chars: &[char], i: usize, needle: &str) -> bool {
    for (idx, c) in (i..).zip(needle.chars()) {
        if idx >= chars.len() {
            return false;
        }
        if !chars[idx].eq_ignore_ascii_case(&c) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_map_to_expected_retryability() {
        assert!(ProviderError::rate_limited("x").is_retryable());
        assert!(ProviderError::network("x").is_retryable());
        assert!(ProviderError::timeout("x").is_retryable());
        assert!(!ProviderError::unauthorized("x").is_retryable());
        assert!(!ProviderError::parse("x").is_retryable());
        assert!(!ProviderError::unsupported("x").is_retryable());
    }

    #[test]
    fn to_info_carries_remediation_and_retry_after() {
        let err = ProviderError::rate_limited("HTTP 429 from upstream")
            .with_retry_after(Some(Duration::from_secs(12)))
            .with_remediation("Wait a moment and refresh")
            .with_doc_url("https://example.test/docs");
        let info = err.to_info();
        assert_eq!(info.kind, ProviderErrorKind::RateLimited);
        assert!(info.retryable);
        assert_eq!(info.retry_after_seconds, Some(12));
        assert_eq!(
            info.remediation.as_deref(),
            Some("Wait a moment and refresh")
        );
        assert_eq!(info.doc_url.as_deref(), Some("https://example.test/docs"));
    }

    #[test]
    fn scrub_redacts_openai_style_keys() {
        let scrubbed = scrub("bad key sk-proj-abcdef0123456789 rejected");
        assert_eq!(scrubbed, "bad key [redacted] rejected");
        assert!(!scrubbed.contains("abcdef"));
    }

    #[test]
    fn scrub_redacts_openrouter_and_google_keys() {
        assert_eq!(scrub("sk-or-v1-0123456789abcdef"), "[redacted]");
        assert_eq!(
            scrub("token AIzaSyA1234567890abcdefg here"),
            "token [redacted] here"
        );
        assert_eq!(scrub("ya29.a0AfH6SMB1234567890"), "[redacted]");
    }

    #[test]
    fn scrub_redacts_jwts() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2lnbmF0dXJl";
        assert_eq!(scrub(&format!("id_token={jwt}")), "id_token=[redacted]");
        assert_eq!(scrub(jwt), "[redacted]");
    }

    #[test]
    fn scrub_redacts_query_parameters_and_bearer_headers() {
        assert_eq!(
            scrub("https://example.test/v1?key=ABCDEFG&alt=json"),
            "https://example.test/v1?key=[redacted]&alt=json"
        );
        assert_eq!(
            scrub("Authorization: Bearer abcdef.ghijkl"),
            "Authorization: Bearer [redacted]"
        );
    }

    #[test]
    fn scrub_leaves_ordinary_prose_alone() {
        let text = "basketball tasks are not sk- prefixed; the sky is blue";
        assert_eq!(scrub(text), text);
        assert_eq!(
            scrub("HTTP 401 from api.anthropic.com"),
            "HTTP 401 from api.anthropic.com"
        );
    }

    #[test]
    fn snippet_truncates_and_flattens() {
        let long = "a".repeat(400);
        let out = snippet(&long);
        assert!(out.chars().count() <= SNIPPET_LIMIT + 1);
        assert!(out.ends_with('…'));

        assert_eq!(snippet("  line one\nline two  "), "line one line two");
    }

    #[test]
    fn snippet_scrubs_echoed_keys() {
        let body = r#"{"error":"invalid key sk-or-v1-1234567890abcdef"}"#;
        let out = snippet(body);
        assert!(!out.contains("1234567890"));
        assert!(out.contains("[redacted]"));
    }

    #[test]
    fn constructor_scrubs_message_at_creation_time() {
        let err = ProviderError::unauthorized("rejected sk-ant-oat01-abcdef123456");
        assert!(!err.message.contains("abcdef123456"));
        assert!(err.message.contains("[redacted]"));
    }
}
