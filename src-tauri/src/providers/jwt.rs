//! Minimal, dependency-free JWT *claim reading*.
//!
//! Codex identifies an account by the `sub` claim of the `id_token` written by
//! `codex login`, and both Claude and Codex carry an `exp` in their access
//! tokens. We only need to read those claims.
//!
//! **Signatures are not verified, and that is correct here.** The token was
//! issued to, and accepted by, the vendor's own CLI on this machine; we are a
//! read-only observer of a local file. We never grant access based on these
//! claims — they are used purely to label a row in the UI and to decide whether
//! to bother making a request that would 401 anyway. Nothing security-relevant
//! depends on them.
//!
//! The decoder is hand-written because the workspace does not depend on a
//! base64 crate and this module must not require a `Cargo.toml` change.

use serde::Deserialize;

use super::error::ProviderError;
use super::secret::Secret;

/// Claims we care about. Everything is optional except `sub`, which is the
/// stable account identifier.
#[derive(Debug, Clone, PartialEq)]
pub struct JwtClaims {
    pub subject: Option<String>,
    pub email: Option<String>,
    /// `exp` in seconds since the Unix epoch.
    pub expires_at: Option<i64>,
    /// ChatGPT plan type, from the `https://api.openai.com/auth` claim block.
    pub chatgpt_plan_type: Option<String>,
    /// ChatGPT account id, from the same block. Opaque, non-secret.
    pub chatgpt_account_id: Option<String>,
}

impl JwtClaims {
    pub fn empty() -> Self {
        JwtClaims {
            subject: None,
            email: None,
            expires_at: None,
            chatgpt_plan_type: None,
            chatgpt_account_id: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawClaims {
    #[serde(default)]
    sub: Option<String>,
    #[serde(default)]
    email: Option<String>,
    /// `exp` is a JSON number that may be serialized as a float.
    #[serde(default)]
    exp: Option<f64>,
    #[serde(default, rename = "https://api.openai.com/auth")]
    openai_auth: Option<OpenAiAuthClaims>,
}

#[derive(Debug, Deserialize)]
struct OpenAiAuthClaims {
    #[serde(default)]
    chatgpt_plan_type: Option<String>,
    #[serde(default)]
    chatgpt_account_id: Option<String>,
}

/// Decode the claim set from a JWT without verifying its signature.
///
/// Takes a [`Secret`] so callers cannot forget that the input is credential
/// material; the returned claims are non-secret identifiers only.
pub fn read_claims(token: &Secret) -> Result<JwtClaims, ProviderError> {
    read_claims_str(token.expose())
}

/// Same as [`read_claims`] for plain strings. Kept `pub(crate)` so tests can
/// use fabricated (non-secret) tokens.
pub(crate) fn read_claims_str(token: &str) -> Result<JwtClaims, ProviderError> {
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(signature)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(ProviderError::parse(
            "credential is not a three-segment JWT",
        ));
    };
    if parts.next().is_some() || header.is_empty() || payload.is_empty() || signature.is_empty() {
        return Err(ProviderError::parse(
            "credential is not a three-segment JWT",
        ));
    }

    let decoded = base64url_decode(payload)
        .ok_or_else(|| ProviderError::parse("JWT payload is not valid base64url"))?;
    let text = String::from_utf8(decoded)
        .map_err(|_| ProviderError::parse("JWT payload is not valid UTF-8"))?;
    let raw: RawClaims = serde_json::from_str(&text)
        .map_err(|_| ProviderError::parse("JWT payload is not valid JSON"))?;

    let (plan, account_id) = match raw.openai_auth {
        Some(auth) => (auth.chatgpt_plan_type, auth.chatgpt_account_id),
        None => (None, None),
    };

    Ok(JwtClaims {
        subject: raw.sub.filter(|s| !s.is_empty()),
        email: raw.email.filter(|s| !s.is_empty()),
        expires_at: raw
            .exp
            .filter(|e| e.is_finite() && *e > 0.0)
            .map(|e| e as i64),
        chatgpt_plan_type: plan.filter(|s| !s.is_empty()),
        chatgpt_account_id: account_id.filter(|s| !s.is_empty()),
    })
}

/// Read only the `exp` claim. Returns `None` for anything unparseable, so
/// callers can fall back to "just try the request".
pub fn expiry_seconds(token: &Secret) -> Option<i64> {
    read_claims(token).ok().and_then(|c| c.expires_at)
}

/// Decode unpadded base64url (RFC 4648 §5). Accepts optional `=` padding.
/// Returns `None` on any invalid character or an impossible length.
pub fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let trimmed = input.trim_end_matches('=');
    let mut out = Vec::with_capacity(trimmed.len() * 3 / 4 + 3);
    let mut accumulator: u32 = 0;
    let mut bits: u32 = 0;

    for ch in trimmed.chars() {
        let value = base64url_value(ch)?;
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((accumulator >> bits) & 0xFF) as u8);
        }
    }

    // A valid encoding leaves fewer than 6 leftover bits, and those bits must
    // all be zero. Anything else is a malformed input, not a truncated one.
    if bits >= 6 {
        return None;
    }
    if bits > 0 && (accumulator & ((1 << bits) - 1)) != 0 {
        return None;
    }
    Some(out)
}

fn base64url_value(ch: char) -> Option<u8> {
    match ch {
        'A'..='Z' => Some(ch as u8 - b'A'),
        'a'..='z' => Some(ch as u8 - b'a' + 26),
        '0'..='9' => Some(ch as u8 - b'0' + 52),
        '-' => Some(62),
        '_' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ProviderErrorKind;

    /// Encode with unpadded base64url so fixtures stay readable in-source.
    fn b64url(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = *chunk.get(1).unwrap_or(&0) as u32;
            let b2 = *chunk.get(2).unwrap_or(&0) as u32;
            let n = (b0 << 16) | (b1 << 8) | b2;
            let indices = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
            let take = match chunk.len() {
                1 => 2,
                2 => 3,
                _ => 4,
            };
            for idx in indices.iter().take(take) {
                out.push(ALPHABET[*idx as usize] as char);
            }
        }
        out
    }

    /// Builds a syntactically valid JWT around a claims payload. The signature
    /// segment is the literal text "not-a-real-signature": no secret material
    /// appears anywhere in this test suite.
    fn fixture_jwt(payload: &str) -> String {
        format!(
            "{}.{}.{}",
            b64url(br#"{"alg":"none","typ":"JWT"}"#),
            b64url(payload.as_bytes()),
            b64url(b"not-a-real-signature"),
        )
    }

    #[test]
    fn base64url_round_trips() {
        for case in [
            &b""[..],
            b"f",
            b"fo",
            b"foo",
            b"foob",
            b"fooba",
            b"foobar",
            b"\x00\xff\x10\x80",
        ] {
            let encoded = b64url(case);
            assert_eq!(
                base64url_decode(&encoded).as_deref(),
                Some(case),
                "round trip for {case:?}"
            );
        }
    }

    #[test]
    fn base64url_accepts_padding_and_rejects_bad_characters() {
        assert_eq!(base64url_decode("Zm9v").as_deref(), Some(&b"foo"[..]));
        assert_eq!(base64url_decode("Zg==").as_deref(), Some(&b"f"[..]));
        assert_eq!(base64url_decode("Zm9v!"), None);
        assert_eq!(base64url_decode("Zm9+"), None); // '+' is standard, not url-safe
        assert_eq!(base64url_decode("A"), None); // 6 leftover bits
    }

    #[test]
    fn reads_codex_identity_claims() {
        let payload = r#"{
            "sub": "user-abc123",
            "email": "dev@example.com",
            "exp": 1893456000,
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "plus",
                "chatgpt_account_id": "acct_9f2c"
            }
        }"#;
        let claims = read_claims_str(&fixture_jwt(payload)).expect("claims parse");
        assert_eq!(claims.subject.as_deref(), Some("user-abc123"));
        assert_eq!(claims.email.as_deref(), Some("dev@example.com"));
        assert_eq!(claims.expires_at, Some(1_893_456_000));
        assert_eq!(claims.chatgpt_plan_type.as_deref(), Some("plus"));
        assert_eq!(claims.chatgpt_account_id.as_deref(), Some("acct_9f2c"));
    }

    #[test]
    fn tolerates_missing_and_empty_claims() {
        let claims = read_claims_str(&fixture_jwt(r#"{"sub":"","exp":0}"#)).expect("parses");
        assert_eq!(claims.subject, None);
        assert_eq!(claims.expires_at, None);
        assert_eq!(claims.chatgpt_plan_type, None);

        let claims = read_claims_str(&fixture_jwt("{}")).expect("parses");
        assert_eq!(claims, JwtClaims::empty());
    }

    #[test]
    fn accepts_float_encoded_exp() {
        let claims = read_claims_str(&fixture_jwt(r#"{"exp":1700000000.0}"#)).expect("parses");
        assert_eq!(claims.expires_at, Some(1_700_000_000));
    }

    #[test]
    fn rejects_malformed_tokens_with_parse_errors() {
        for bad in ["", "abc", "a.b", "a.b.c.d", "..", "a..c"] {
            let err = read_claims_str(bad).expect_err("should reject");
            assert_eq!(err.kind, ProviderErrorKind::Parse, "input {bad:?}");
        }
    }

    #[test]
    fn rejects_non_json_payloads() {
        let token = format!(
            "{}.{}.{}",
            b64url(b"{}"),
            b64url(b"not json at all"),
            b64url(b"sig")
        );
        let err = read_claims_str(&token).expect_err("should reject");
        assert_eq!(err.kind, ProviderErrorKind::Parse);
    }

    #[test]
    fn expiry_helper_is_forgiving() {
        assert_eq!(expiry_seconds(&Secret::new("garbage")), None);
        let token = fixture_jwt(r#"{"exp":1700000000}"#);
        assert_eq!(expiry_seconds(&Secret::new(token)), Some(1_700_000_000));
    }

    #[test]
    fn claims_never_include_the_raw_token() {
        let token = fixture_jwt(r#"{"sub":"s","email":"e@example.com"}"#);
        let claims = read_claims_str(&token).expect("parses");
        let rendered = format!("{claims:?}");
        assert!(!rendered.contains(&token));
    }
}
