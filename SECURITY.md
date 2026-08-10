# Security policy

EyeUrAI reads sensitive account metadata, so its security boundary is intentionally narrow.

## Security model

- The app has no hosted backend and no EyeUrAI user account.
- Secrets must remain in provider-owned or operating-system credential storage.
- The React webview receives normalized quota metadata, never raw credentials.
- Logs must redact authorization headers and account tokens.
- Provider access is read-only.
- Cached snapshots must not contain prompts, responses, files, or code.
- Retained account snapshots contain only normalized quota metadata, use pseudonymous account IDs, and are stored in owner-only files. OAuth credentials and refresh tokens are never copied into the snapshot registry.

## Reporting a vulnerability

Please use GitHub's private **Security Advisories** feature for this repository. Do not open a public issue for credential exposure, authentication bypasses, unsafe token handling, or dependency vulnerabilities with a working exploit.

Include the affected version, platform, reproduction steps, and impact. Use synthetic credentials in all reports.
