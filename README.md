# EyeUrAI

See every AI limit at a glance.

EyeUrAI is an open-source, local-first menu-bar and system-tray app for monitoring AI subscription quotas and API credits. Launch it when you need it, click the eye icon, and see every connected provider and account in one compact view.

> EyeUrAI is in active development. Provider quota interfaces change frequently; every adapter reports its freshness and fails visibly instead of inventing a number.

## Installation

EyeUrAI is currently installed from source. You will need:

- macOS or Windows
- [Git](https://git-scm.com/downloads)
- Node.js 24 or newer and npm 11 or newer
- [Rust stable](https://www.rust-lang.org/tools/install)
- The [Tauri system prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform
- At least one supported provider CLI already installed and signed in

### Clone and run

Clone the repository, install its dependencies, and launch the desktop app:

```sh
git clone https://github.com/itsnotmarvin/eyeurai.git
cd eyeurai
npm install
npm run desktop:dev
```

EyeUrAI opens as an eye icon in the macOS menu bar or Windows system tray. Click the icon to open the quota panel. The terminal process must remain open while running this development build; press `Ctrl+C` in the terminal to stop it.

### Build the desktop app

To create a standalone app that does not need a terminal running:

```sh
npm run desktop:build
```

Tauri places the platform installer and app bundle under `src-tauri/target/release/bundle/`. On macOS, open the generated DMG and drag EyeUrAI into Applications. On Windows, run the generated installer.

Development builds started with `npm run desktop:dev` do not check for updates. After the first signed public release, install EyeUrAI from GitHub Releases once; that version will show an **Update available** button for later releases.

### Provider sign-in

EyeUrAI reuses provider credentials that already exist on your computer. It does not ask for subscription passwords or browser cookies.

```sh
# OpenAI / Codex
# Add accounts from EyeUrAI Settings → Add account. EyeUrAI opens the
# official Codex browser flow in a separate profile for each account.

# Anthropic / Claude
claude auth login
```

For OpenRouter, launch EyeUrAI from a terminal where `OPENROUTER_API_KEY` or `OPENROUTER_KEY` is set. Gemini usage percentages remain unavailable until Google exposes a supported API for them.

## Updates

Release builds check for signed updates when EyeUrAI starts, when its window regains focus after at least 15 minutes, and every four hours while it is running. When a newer release exists, the header shows **Update available**.

Select that button and choose **Update & restart**. EyeUrAI downloads and verifies the release, closes, installs it, and reopens automatically. Update checks never run in the browser preview or `desktop:dev` development build.

## What it shows

- Claude five-hour, weekly, and model-specific plan windows when available
- OpenAI/Codex primary and secondary quota windows
- OpenRouter credits and key limits
- Gemini connection status, with an explicit unsupported state until Google exposes current usage percentages through a supported API
- Multiple personal and work accounts per provider
- Percentage-used bars, reset countdowns, stale/error states, and optional alerts

The first release deliberately does **not** collect conversations, calculate productivity scores, draw historical graphs, switch active credentials, sync data to a cloud service, or start automatically at login.

## Privacy

EyeUrAI has no account system, analytics, telemetry, or hosted backend.

- Provider requests go directly from your computer to that provider.
- Prompts, responses, files, and source code are never collected.
- UI snapshots contain only quota metadata such as percentages and reset times.
- Secrets remain in the provider or operating-system credential store and are never sent to the webview.
- Removing an EyeUrAI account removes its local configuration.

See [SECURITY.md](SECURITY.md) for the security model and reporting process.

## Platforms

- macOS: menu-bar popover
- Windows: system-tray popover
- Linux: planned after the first macOS and Windows release

EyeUrAI does not launch at login. After restarting your computer, open EyeUrAI normally to put its icon back in the menu bar or system tray.

## Download

Release builds and source archives are available from [GitHub Releases](https://github.com/itsnotmarvin/eyeurai/releases/latest).

The 1.0.0 prebuilt app is for Apple silicon Macs and is currently unsigned. Windows is supported from source; a signed Windows installer is not included in this release.

## Connecting providers

This first implementation reuses credentials that already exist on your computer; it does not ask you to paste subscription passwords or copy browser cookies.

| Provider | Current connection | What EyeUrAI can show |
| --- | --- | --- |
| Claude | The OAuth login written by Claude Code | Five-hour, weekly, and model-specific percentages and resets |
| OpenAI / Codex | The existing `~/.codex` login plus isolated profiles added in EyeUrAI | Independently refreshed primary/secondary subscription windows for multiple accounts |
| OpenRouter | `OPENROUTER_API_KEY` or `OPENROUTER_KEY` when EyeUrAI is launched | The documented current-key USD spend ceiling and reset cadence |
| Gemini | Google project configuration is detected when available | A clear unavailable state; Google currently directs users to AI Studio for active rate limits and does not expose the account-wide percentage this app needs |

Claude and the default Codex connection use read-only first-party endpoints also used by their official clients, but those routes are not documented public APIs and may change. OpenRouter uses its documented current-key API. EyeUrAI never refreshes or rewrites the default terminal login. Codex accounts added inside EyeUrAI use the official Codex app-server, which owns browser login, credential persistence, and refresh-token rotation inside a separate `CODEX_HOME` for each account.

The data model and UI support accounts from multiple providers at the same time. Automatic discovery assigns Claude and Codex logins stable, pseudonymous account IDs and retains the last successful quota snapshot when a terminal login changes. Codex accounts added through EyeUrAI each have an isolated provider-owned profile, so multiple Codex accounts can stay signed in and refresh live without copying credentials between them. Retained snapshots are clearly marked as last-known data.

## Development

### Run only the interface

```sh
npm install
npm run dev
```

The ordinary Vite view uses deterministic demo data so interface work never requires real credentials. It is intended for interface development, not normal use. Demo data is never substituted by the packaged app's live commands.

### Run the desktop app

```sh
npm run desktop:dev
```

### Verify

```sh
npm test
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml
```

### Publish a release

Releases remain fully open source: the source, workflow, release notes, public verification key, and installers are public. Only the private updater key stays secret.

A private updater key was generated locally at `~/.tauri/eyeurai.key` and added to the GitHub repository as `TAURI_SIGNING_PRIVATE_KEY`. Back up the local key securely. If the secret ever needs to be restored, use this command without printing or committing it:

```sh
gh secret set TAURI_SIGNING_PRIVATE_KEY \
  --repo itsnotmarvin/eyeurai \
  < ~/.tauri/eyeurai.key
```

Losing this key prevents new updates from being installed by existing users. The matching public key in `src-tauri/tauri.conf.json` is intentionally public.

For each release, choose a version greater than the currently published version. The preparation command updates `package.json`, `package-lock.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, and `src-tauri/Cargo.lock` together:

```sh
npm run release:prepare -- 1.0.1
npm run release:check
npm test
npm run build

git add .
git commit -m "Release EyeUrAI 1.0.1"
git tag v1.0.1
git push origin main --follow-tags
```

Pushing the `v1.0.1` tag runs `.github/workflows/release.yml`. GitHub Actions tests the project, builds macOS Apple Silicon, macOS Intel, and Windows installers, signs the updater artifacts, publishes the GitHub Release, and generates `latest.json`. Installed copies then discover the release automatically.

The current macOS builds use ad-hoc code signing so the open-source release pipeline can run without private Apple credentials. Before distributing broadly, Apple Developer signing and notarization are recommended to remove Gatekeeper friction. Updater signing and Apple signing are separate protections.

## Architecture

- `src/` — React/TypeScript popover, onboarding, settings, and accessibility
- `src-tauri/src/providers/` — isolated provider adapters
- `src-tauri/src/models.rs` — provider-neutral quota contract
- `src-tauri/src/commands.rs` — narrow Tauri command boundary
- `src-tauri/src/lib.rs` — tray lifecycle and popover positioning

Provider-specific data is normalized into account snapshots containing one or more quota windows. The frontend never needs to know how credentials are stored or how a provider fetches its limits.

## Contributing

Provider adapters are the most useful contribution. Please read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request. New adapters must use read-only provider access, keep secrets out of snapshots and logs, include fixture-based parser tests, and represent unsupported data honestly.

## Trademark notice

EyeUrAI is an independent open-source project and is not affiliated with or endorsed by Anthropic, OpenAI, OpenRouter, Google, Cursor, or any other provider. Provider names and marks belong to their respective owners.

## License

[MIT](LICENSE)
