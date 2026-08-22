# EyeUrAI

See every AI limit at a glance.

EyeUrAI is an open-source, local-first menu-bar and system-tray app for monitoring AI subscription quotas and API credits. Launch it when you need it, click the eye icon, and see every connected provider and account in one compact view.

> EyeUrAI is in active development. Provider quota interfaces change frequently; every adapter reports its freshness and fails visibly instead of inventing a number.

## Installation

Download EyeUrAI only from the [latest GitHub Release](https://github.com/itsnotmarvin/eyeurai/releases/latest). Files ending in `.sig` or `.app.tar.gz`, plus `latest.json`, are used by the automatic updater; you do not need them for a normal installation.

### Download and install on macOS

EyeUrAI requires macOS 11 or newer.

1. On the release page, download the DMG that matches your Mac:
   - **Apple silicon (M1, M2, M3, M4, or newer):** `EyeUrAI_<version>_aarch64.dmg`
   - **Intel:** `EyeUrAI_<version>_x64.dmg`
2. Open the downloaded DMG.
3. Drag **EyeUrAI** into the **Applications** folder.
4. Open **EyeUrAI** from Applications. The eye icon will appear in the macOS menu bar.

If you are unsure which Mac you have, open **Apple menu → About This Mac** and look for **Chip** (Apple silicon) or **Processor** (Intel).

The current macOS builds use ad-hoc code signing. If macOS blocks the first launch, open **System Settings → Privacy & Security**, confirm that the blocked app is EyeUrAI, and choose **Open Anyway**.

### Download and install on Windows

1. On the release page, download `EyeUrAI_<version>_x64-setup.exe`.
2. Open the downloaded installer and follow its prompts.
3. Launch **EyeUrAI** from the Start menu. The eye icon will appear in the Windows system tray; it may be inside the **Show hidden icons** menu (`^`).

The Windows installer is not yet Authenticode-signed, so Microsoft Defender SmartScreen may warn on first launch. Verify that the installer came from this repository's official GitHub Release, then choose **More info → Run anyway** to continue. Tauri updater artifacts are signed separately so installed copies can verify automatic updates.

### Install from source

To build EyeUrAI yourself, you will need:

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

Development builds started with `npm run desktop:dev` do not check for updates. Install EyeUrAI from GitHub Releases once; that version will show an **Update available** button for later releases.

## Resource use

EyeUrAI is designed to remain lightweight while it sits in the menu bar or system tray. In one macOS idle measurement, the app and its WebKit helper processes averaged about 2% of one CPU core, were usually at 0% CPU between brief updates, and used roughly 50–190 MB of memory depending on how macOS accounted for shared WebKit memory. The installed application was about 7 MB. Actual usage varies by platform, account count, and how often the panel is opened or refreshed.

Quota checks run when the app loads and automatically at the interval selected under **Settings → Usage updates**. The default is once a minute; available intervals range from 15 seconds to 5 minutes. Signed release builds check for app updates only at the intervals described below. Optional local-usage analysis can briefly use additional CPU and disk I/O while reading local Claude Code and Codex session logs.

Building from source is substantially heavier than running the installed app. The first Rust build may use several CPU cores and create several gigabytes of compiler artifacts under `src-tauri/target/`; development mode also keeps the Vite and Rust development processes running. These build artifacts consume disk space, not continuous runtime power, and can be removed with `cargo clean --manifest-path src-tauri/Cargo.toml` when they are no longer needed.

### Provider sign-in

EyeUrAI reuses provider credentials that already exist on your computer, and can add further accounts through each provider's official browser sign-in. It never asks for subscription passwords or browser cookies.

```sh
# OpenAI / Codex
# Add accounts from EyeUrAI Settings → Add account. EyeUrAI opens the
# official Codex browser flow in a separate profile for each account.

# Anthropic / Claude
# Add accounts from EyeUrAI Settings → Add account. EyeUrAI opens
# Anthropic's official browser sign-in in a separate profile for each
# account. An existing terminal login is also picked up automatically:
claude /login
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
- Multiple live Claude and Codex accounts side by side, plus retained snapshots when a terminal login changes
- Percentage-used bars, reset countdowns, stale/error states, and optional alerts

EyeUrAI deliberately does **not** collect conversations, calculate productivity scores, switch active credentials, or sync data to a cloud service. With explicit permission, it can summarize token counters from local Claude Code and Codex logs and draw rolling daily usage graphs; message content and file paths never reach the interface. Fresh installations enable launch at login by default so the menu-bar reading stays available after a restart, and this can be disabled at any time in Settings.

## Privacy

EyeUrAI has no account system, analytics, telemetry, or hosted backend.

- Provider requests go directly from your computer to that provider.
- Prompts, responses, files, and source code are never collected.
- UI snapshots contain only quota metadata such as percentages and reset times.
- Secrets stay out of the interface: they remain in the provider or operating-system credential store, or — for accounts added in EyeUrAI — in that account's owner-only profile directory, and are never sent to the webview.
- Claude accounts added in EyeUrAI are granted a read-only scope, so the stored tokens can read usage but can never run Claude.
- Removing an EyeUrAI account removes its local configuration.

### Every network connection

The interface itself cannot reach the internet: its content-security policy only allows talking to the local Tauri bridge, and all fonts, icons, and provider logos ship inside the app. Network requests are made only by the Rust backend, using your own credentials, and only to:

- `api.anthropic.com`, `claude.com`, and `platform.claude.com` — Claude usage and sign-in
- `chatgpt.com` and `auth.openai.com` — Codex usage and sign-in
- `openrouter.ai` — OpenRouter key status
- `generativelanguage.googleapis.com` — Gemini connection check
- GitHub Releases — signed update checks, in installed release builds only

Nothing else is contacted. Because there is no hosted backend, every installation is fully self-contained: your tokens, usage data, and preferences exist only on your machine.

See [SECURITY.md](SECURITY.md) for the security model and reporting process.

## Platforms

- macOS: menu-bar popover
- Windows: system-tray popover
- Linux: planned after the first macOS and Windows release

Fresh installations launch EyeUrAI at login by default, hidden in the menu bar or system tray. You can turn **Launch at login** off or back on in Settings; existing installations preserve the choice they already made.

## Download

Release builds and source archives are available from [GitHub Releases](https://github.com/itsnotmarvin/eyeurai/releases/latest).

The current release includes DMGs for Apple silicon and Intel Macs plus an x64 Windows installer. macOS 11 or newer is required. Platform signing status and any expected operating-system warnings are documented in the installation section above.

## Connecting providers

This first implementation reuses credentials that already exist on your computer; it does not ask you to paste subscription passwords or copy browser cookies.

| Provider | Current connection | What EyeUrAI can show |
| --- | --- | --- |
| Claude | The OAuth login written by Claude Code plus isolated accounts added in EyeUrAI | Independently refreshed five-hour, weekly, and model-specific percentages and resets for multiple accounts |
| OpenAI / Codex | The existing `~/.codex` login plus isolated profiles added in EyeUrAI | Independently refreshed primary/secondary subscription windows for multiple accounts |
| OpenRouter | `OPENROUTER_API_KEY` or `OPENROUTER_KEY` when EyeUrAI is launched | The documented current-key USD spend ceiling and reset cadence |
| Gemini | Google project configuration is detected when available | A clear unavailable state; Google currently directs users to AI Studio for active rate limits and does not expose the account-wide percentage this app needs |

Claude and the default Codex connection use read-only first-party endpoints also used by their official clients, but those routes are not documented public APIs and may change. OpenRouter uses its documented current-key API. EyeUrAI never refreshes or rewrites the default terminal logins. Codex accounts added inside EyeUrAI use the official Codex app-server, which owns browser login, credential persistence, and refresh-token rotation inside a separate `CODEX_HOME` for each account. Claude accounts added inside EyeUrAI sign in through Anthropic's official browser flow — the same first-party OAuth client Claude Code uses — into a separate EyeUrAI profile per account; EyeUrAI requests a read-only scope, stores each grant only in that profile, and rotates it itself without ever touching the terminal login.

The data model and UI support accounts from multiple providers at the same time. Automatic discovery assigns Claude and Codex logins stable, pseudonymous account IDs and retains the last successful quota snapshot when a terminal login changes. Claude and Codex accounts added through EyeUrAI each have an isolated profile, so multiple accounts can stay signed in and refresh live without copying credentials between them. Adding the same account the terminal already uses shows one row, preferring the independently refreshed profile. Retained snapshots are clearly marked as last-known data.

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
npm run release:prepare -- 1.1.1
npm run release:check
npm test
npm run build

git add .
git commit -m "Release EyeUrAI 1.1.1"
git tag v1.1.1
git push origin main --follow-tags
```

Pushing the `v1.1.1` tag runs `.github/workflows/release.yml`. GitHub Actions tests the project, builds macOS Apple Silicon, macOS Intel, and Windows installers, signs the updater artifacts, publishes the GitHub Release, and generates `latest.json`. Installed copies then discover the release automatically.

The current macOS builds use ad-hoc code signing so the open-source release pipeline can run without private Apple credentials. Before distributing broadly, Apple Developer signing and notarization are recommended to remove Gatekeeper friction. Updater signing and Apple signing are separate protections.

## Architecture

- `src/` — React/TypeScript popover, onboarding, settings, and accessibility
- `src-tauri/src/providers/` — isolated provider adapters
- `src-tauri/src/models.rs` — provider-neutral quota contract
- `src-tauri/src/commands.rs` — narrow Tauri command boundary
- `src-tauri/src/account_registry.rs` — atomic, secret-free retained account snapshots
- `src-tauri/src/claude_profiles.rs` and `codex_profiles.rs` — isolated multi-account profile discovery and login
- `src-tauri/src/local_usage.rs` — bounded, aggregate-only scans of local CLI token counters
- `src-tauri/src/lib.rs` — tray lifecycle and popover positioning

Provider-specific data is normalized into account snapshots containing one or more quota windows. The frontend never needs to know how credentials are stored or how a provider fetches its limits.

## Contributing

Provider adapters are the most useful contribution. Please read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request. New adapters must use read-only provider access, keep secrets out of snapshots and logs, include fixture-based parser tests, and represent unsupported data honestly.

## Trademark notice

EyeUrAI is an independent open-source project and is not affiliated with or endorsed by Anthropic, OpenAI, OpenRouter, Google, Cursor, or any other provider. Provider names and marks belong to their respective owners.

## License

[MIT](LICENSE)
