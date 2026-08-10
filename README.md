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
git clone https://github.com/YOUR_GITHUB_ACCOUNT/eyeurai.git
cd eyeurai
npm install
npm run desktop:dev
```

Replace `YOUR_GITHUB_ACCOUNT` with the account or organization where the public repository is hosted. The final repository URL will be placed here before the first public release.

EyeUrAI opens as an eye icon in the macOS menu bar or Windows system tray. Click the icon to open the quota panel. The terminal process must remain open while running this development build; press `Ctrl+C` in the terminal to stop it.

### Build the desktop app

To create a standalone app that does not need a terminal running:

```sh
npm run desktop:build
```

Tauri places the platform installer and app bundle under `src-tauri/target/release/bundle/`. On macOS, open the generated DMG and drag EyeUrAI into Applications. On Windows, run the generated installer.

These source builds do not update automatically. After the first signed public release, install EyeUrAI from GitHub Releases once; that version will be able to show an **Update available** button for later releases.

### Provider sign-in

EyeUrAI reuses provider credentials that already exist on your computer. It does not ask for subscription passwords or browser cookies.

```sh
# OpenAI / Codex
codex login

# Anthropic / Claude
claude auth login
```

For OpenRouter, launch EyeUrAI from a terminal where `OPENROUTER_API_KEY` or `OPENROUTER_KEY` is set. Gemini usage percentages remain unavailable until Google exposes a supported API for them.

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
| OpenAI / Codex | The ChatGPT subscription login in `~/.codex/auth.json` | Primary/secondary subscription windows, named by their actual duration |
| OpenRouter | `OPENROUTER_API_KEY` or `OPENROUTER_KEY` when EyeUrAI is launched | The documented current-key USD spend ceiling and reset cadence |
| Gemini | Google project configuration is detected when available | A clear unavailable state; Google currently directs users to AI Studio for active rate limits and does not expose the account-wide percentage this app needs |

Claude and Codex use read-only first-party endpoints also used by their official clients, but those routes are not documented public APIs and may change. OpenRouter uses its documented current-key API. EyeUrAI never refreshes or rewrites a provider login.

The data model and UI support multiple accounts. Automatic discovery assigns the active Claude and Codex login a stable, pseudonymous account ID and retains each account's last successful quota snapshot when the terminal switches to another login. Retained snapshots are clearly marked as last-known data: EyeUrAI does not copy or race the CLI's rotating refresh token, so only the currently available CLI login can be refreshed live. Provider-owned isolated logins are the next connection milestone.

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
