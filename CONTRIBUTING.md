# Contributing to EyeUrAI

Thanks for helping make AI quota information easier to see.

## Ground rules

- Never commit credentials, session cookies, OAuth tokens, provider exports, prompts, or conversation content.
- Do not add scraping or reverse-engineered browser automation without prior maintainer discussion.
- A provider adapter must time out, classify errors, preserve last-known data, and return `unsupported` instead of guessing.
- Keep macOS and Windows behavior in mind even when developing on one platform.
- Avoid telemetry and new network destinations. Provider calls should remain easy to audit.

## Setup

```sh
npm install
npm run build
npm test
cargo test --manifest-path src-tauri/Cargo.toml
```

Run the full desktop shell with:

```sh
npm run desktop:dev
```

## Adding a provider

1. Implement the provider boundary under `src-tauri/src/providers/`.
2. Normalize results into the shared quota model.
3. Add parser tests using synthetic fixtures with obviously fake credentials.
4. Document whether the data source is official, provider-client-owned, or inferred.
5. Include graceful behavior for unavailable fields and rate limiting.
6. Add frontend branding without shipping proprietary artwork that cannot be redistributed.

## Pull requests

Keep pull requests focused. Explain the data source, authentication method, security implications, test coverage, and behavior when the provider changes or becomes unavailable.
