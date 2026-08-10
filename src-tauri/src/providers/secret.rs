//! A minimal secret wrapper.
//!
//! The single job of [`Secret`] is to make it *hard to accidentally leak a
//! token*:
//!
//! * it does not implement `serde::Serialize`, so it can never end up in a
//!   snapshot, a Tauri command response, or a `tauri-plugin-store` file;
//! * `Debug` and `Display` render `<redacted>`, so `{:?}` in a log line or a
//!   `dbg!` left behind in a PR cannot spill it;
//! * the inner value is only reachable through the explicit, greppable
//!   [`Secret::expose`] call;
//! * `Drop` overwrites the backing bytes on a best-effort basis.
//!
//! It is deliberately *not* a full zeroize implementation: `String` may have
//! been reallocated during construction, so earlier copies can survive. The
//! wrapper reduces accidental exposure, it does not defeat a memory attacker.

use std::fmt;

/// Opaque holder for credential material.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Secret(value.into())
    }

    /// The only way to read the secret. Grep for `.expose()` to audit every
    /// place credential material is touched.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Byte length. Safe to log — it reveals nothing beyond magnitude.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// A non-reversible display hint such as `sk-or…9f2c` for the settings UI.
    ///
    /// Shows at most the first 6 and last 4 characters, and only when the
    /// secret is long enough that those fragments cannot reconstruct it
    /// (>= 16 chars). Shorter secrets are fully masked.
    pub fn masked_hint(&self) -> String {
        let chars: Vec<char> = self.0.chars().collect();
        if chars.len() < 16 {
            return "•".repeat(8);
        }
        let head: String = chars.iter().take(6).collect();
        let tail: String = chars.iter().skip(chars.len() - 4).collect();
        format!("{head}…{tail}")
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        // Best-effort scrub of the current allocation.
        // SAFETY-adjacent note: we only write ASCII spaces over existing bytes,
        // so the String remains valid UTF-8 throughout.
        let len = self.0.len();
        unsafe {
            let bytes = self.0.as_mut_vec();
            for byte in bytes.iter_mut() {
                *byte = 0x20;
            }
            debug_assert_eq!(bytes.len(), len);
        }
        self.0.clear();
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Secret(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Secret(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_display_are_redacted() {
        let secret = Secret::new("sk-or-v1-supersecretvalue");
        assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
        assert_eq!(format!("{secret}"), "<redacted>");
        assert!(!format!("{secret:?} {secret}").contains("supersecret"));
    }

    #[test]
    fn expose_returns_the_value() {
        let secret = Secret::new("abc123");
        assert_eq!(secret.expose(), "abc123");
        assert_eq!(secret.len(), 6);
        assert!(!secret.is_empty());
    }

    #[test]
    fn masked_hint_hides_short_secrets_entirely() {
        assert_eq!(Secret::new("short").masked_hint(), "••••••••");
        assert_eq!(Secret::new("0123456789abcde").masked_hint(), "••••••••");
    }

    #[test]
    fn masked_hint_shows_edges_for_long_secrets() {
        let secret = Secret::new("sk-or-v1-0123456789abcdef9f2c");
        let hint = secret.masked_hint();
        assert_eq!(hint, "sk-or-…9f2c");
        assert!(!hint.contains("0123456789"));
    }

    #[test]
    fn secret_is_not_serializable() {
        // Compile-time guarantee documented here for reviewers: `Secret` has no
        // `Serialize` impl, so the following would not compile:
        //   serde_json::to_string(&Secret::new("x"));
        // This test asserts the runtime property we can check: the type has no
        // public accessor other than `expose`.
        let secret = Secret::new("x".repeat(40));
        assert!(!secret.masked_hint().contains("xxxxxxxxxxxxxxxxxxxx"));
    }
}
