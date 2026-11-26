# Ombra Manager (Ombra Desktop)

Local, open-source anti-detect profile manager and browser workspace built with Tauri, Rust, and Vue 3. Runs 100% locally with no account sign-in or external licensing server required.

For the compiled **Ombra Chromium** browser binaries and Node.js / Python automation SDKs, see **[ombralabs/ombra](https://github.com/ombralabs/ombra)**.

## Features

- **Profiles, Groups, Tags & Trash:** Full SQLite-backed profile catalog with customizable browser fingerprints (OS, GPU, CPU/RAM, screen, timezone, language, WebRTC, canvas/audio noise, geolocation, storage quota), humanize persona presets and preview (`ombra-humanize`), credentials & built-in TOTP generator, start URLs, bookmarks, custom flags, bulk create/edit/clone, CSV import/export, encrypted archive import/export (`.tar.gz`), and soft-delete trash.
- **Browser Engine Management:** Downloads Ed25519-signed Ombra Chromium releases directly from public [GitHub Releases (`ombralabs/ombra`)](https://github.com/ombralabs/ombra/releases) with SHA-256 verification, immutable version folders (`engines/versions/<version>/`), version switching, rollback, and local archive import. Supports Chromium 158+ (`Ombra.app` / `ombra` / `ombra.exe`) and legacy `Chromium.app` / `chrome` layouts.
- **Proxies, Local Relay & GeoIP:** HTTP / HTTPS / SOCKS5 proxy manager with bulk import, health & GeoIP checks (auto-resolving timezone/language/WebRTC), direct-connection mode, exclusive proxy locks, local per-profile authenticating relay, and live per-profile traffic & connection logs.
- **Local API (AdsPower v1/v2 Compatible):** Localhost HTTP API (`127.0.0.1`) with optional API key for starting/stopping profiles, querying CDP WebSocket endpoints, managing profiles/groups/cookies/extensions, and driving sessions from Playwright/Puppeteer.
- **Window Synchronizer:** Mirrors mouse clicks, typing, scrolling, navigation, tabs, and page dialogs from a master browser window to follower windows in **Full Copy** or **Varied** (randomized human-like delays) mode, with per-window pause/release/resume.
- **Visual RPA & CLI (`ombra flow`):** Node-based visual workflow editor, template library, multi-threaded task runner, CSV/Excel/Google Sheets table data sources, execution journal, live debugger, and standalone headless CLI (`ombra flow`).
- **Extensions & Cookies:** Extension library (Chrome Web Store download + local `.crx`/`.zip`/unpacked folder), extension sets per profile/group, and JSON/Netscape cookie import/export with live CDP injection.
- **Local Encryption & Security:** Per-secret encryption with local key storage (see [docs/local-keys.md](docs/local-keys.md)) and optional macOS encrypted APFS vault with TOTP 2FA (see [docs/profile-security.md](docs/profile-security.md)).

## Development

Prerequisites: Node.js 22+, Rust toolchain from [`rust-toolchain.toml`](rust-toolchain.toml), and [Tauri v2 system prerequisites](https://v2.tauri.app/start/prerequisites/).

```sh
npm ci
npm run check:web
cargo test --locked --manifest-path src-tauri/Cargo.toml
cargo clippy --locked --all-targets --manifest-path src-tauri/Cargo.toml -- -D warnings
```

Run the desktop app in development mode:

```sh
npm run tauri -- dev
```

Build release installers (see [docs/BUILDING.md](docs/BUILDING.md)):

```sh
npm run build:desktop -- darwin-arm64   # or darwin-x64, windows-x64, linux-x64
```

## Documentation

- [docs/BUILDING.md](docs/BUILDING.md) — Building installers and engine release format
- [docs/automation-cli.md](docs/automation-cli.md) — `ombra flow` CLI reference
- [docs/automation-cdp-methods.md](docs/automation-cdp-methods.md) — CDP methods used by automation
- [docs/local-keys.md](docs/local-keys.md) — Local encryption key storage
- [docs/profile-security.md](docs/profile-security.md) — Encrypted macOS APFS vault and 2FA
- [docs/humanize-manual-check.md](docs/humanize-manual-check.md) — Humanize input verification guide

## License

Licensed under the **GNU General Public License v3.0 only (`GPL-3.0-only`)**. See [LICENSE](LICENSE) and [NOTICE](NOTICE).

Compiled Ombra Chromium engine binaries downloaded at runtime from `ombralabs/ombra` are separate works distributed under their own license terms.
