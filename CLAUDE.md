# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Nutune is a CLI tool that syncs music from Subsonic servers to portable music devices (DAPs). It features device auto-detection, interactive TUI browsing, parallel downloads, and manifest-based sync tracking.

## Build & Development Commands

```bash
# Enter development environment (Nix)
nix develop

# Build
cargo build
cargo build --release

# Run
cargo run -- <command>

# Lint
cargo clippy

# Format
cargo fmt

# Build Nix package
nix build
```

## Architecture

### Module Structure

- **cli/** - Command-line interface with clap
  - `mod.rs` - CLI struct and subcommand definitions
  - `commands.rs` - Command handler implementations
  - `auth.rs` - Keyring-based credential management

- **subsonic/** - Subsonic REST API integration
  - `client.rs` - HTTP client wrapper (reqwest-based async)
  - `auth.rs` - MD5-based auth parameter generation
  - `models.rs` - API response deserialization types

- **device/** - Device management
  - `detection.rs` - Device detection via `lsblk`/`udisksctl`
  - `manifest.rs` - SyncManifest tracks synced content per device
  - `storage.rs` - Filesystem operations on target device

- **sync/** - Download orchestration
  - `engine.rs` - Main sync coordination
  - `downloader.rs` - Parallel async download manager with progress bars

- **browse/** - Interactive TUI
  - `interactive.rs` - ratatui-based music browser (Artists/Playlists views)

- **utils/** - Helpers
  - `cover_art.rs` - Cover art processing and embedding (lofty + image crates)
  - `m3u.rs` - M3U playlist generation
  - `sanitize.rs` - Filename sanitization

### Data Flow

1. User authenticates via `auth` command → credentials stored in system keyring
2. `browse` command → Interactive TUI fetches library from Subsonic API → user selects content
3. `sync` command → SyncEngine checks manifest → Downloader fetches songs in parallel → writes to device with embedded cover art
4. Manifest updated to track synced albums/playlists

### Key Patterns

- Async-first with Tokio runtime
- Error handling: `anyhow::Result` for propagation, `thiserror` for custom errors
- All HTTP operations through `SubsonicClient` in `subsonic/client.rs`
- Device state tracked via `SyncManifest` JSON files on device

## Environment Variables

- `SUBSONIC_URL` - Subsonic server URL
- `SUBSONIC_USER` - Username
- `SUBSONIC_PASS` - Password
- `RUST_LOG` - Logging level (tracing-subscriber)
