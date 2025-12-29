# Nutune

Sync music from Subsonic servers to portable music devices (DAPs).

## Features

- **Device auto-detection** - Automatically detects connected portable devices
- **Interactive TUI browser** - Browse artists and playlists with a keyboard-driven interface
- **Parallel downloads** - Fast syncing with concurrent downloads and progress bars
- **Manifest-based tracking** - Remembers what's already synced to avoid re-downloading
- **Cover art embedding** - Automatically embeds album art into synced files
- **M3U playlist generation** - Creates playlists compatible with your device

## Installation

### Nix (recommended)

```bash
nix build
# or run directly
nix run
```

### Cargo

```bash
cargo build --release
```

## Usage

### Authentication

Store your Subsonic credentials securely in the system keyring:

```bash
nutune auth
```

Or use environment variables:

```bash
export SUBSONIC_URL=https://your-server.com
export SUBSONIC_USER=username
export SUBSONIC_PASS=password
```

### Browse and Sync

Launch the interactive browser to select music:

```bash
nutune browse
```

Sync selected content to your device:

```bash
nutune sync
```

## Requirements

- A Subsonic-compatible server (Subsonic, Navidrome, Airsonic, etc.)
- A mounted portable music device

