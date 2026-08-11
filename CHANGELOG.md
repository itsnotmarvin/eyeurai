# Changelog

All notable changes to EyeUrAI are documented in this file.

## Unreleased

- Add isolated Claude accounts: sign in additional claude.ai accounts through Anthropic's official browser flow and watch every account's usage side by side, independent of the terminal login.
- Request Claude account grants with a read-only scope so tokens EyeUrAI stores can see usage but never run inference; grants are refreshed and rotated inside each owner-only profile.
- Prefer the independently managed profile when a terminal login and an added account are the same account, for both Claude and Codex.
- Find the Codex CLI on Windows (`codex.exe` / `codex.cmd`, npm's global directory) and let npm-shim installs launch by passing `PATH` and the command-interpreter variables to the app-server.

## 1.1.0 — 2026-08-10

- Add isolated Codex profiles so multiple personal and work accounts can remain signed in simultaneously.
- Use the official Codex app-server browser flow to own login, credential persistence, and refresh-token rotation inside each profile.
- Preserve non-secret last-known quota snapshots when terminal logins change while keeping profile credentials isolated.
- Publish Apple silicon and Intel macOS builds, a Windows x64 installer, and signed automatic-update artifacts.

## 1.0.0 — 2026-08-09

- Monitor Claude, OpenAI/Codex, OpenRouter, and Gemini connection states from a compact desktop popover.
- Pin any available account quota window to the menu bar for a concise live percentage display.
- Inspect opt-in local Claude Code and Codex token usage over 7, 30, or 90 calendar days.
- Configure provider visibility, notification thresholds, and retained account connections.
- Keep credentials local and send provider requests directly from the desktop app without telemetry or a hosted backend.

The downloadable 1.0.0 app bundle is an unsigned Apple silicon macOS build. Windows remains available as a source build for this release.
