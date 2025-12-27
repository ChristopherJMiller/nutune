//! Sync manifest tracking for devices

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::debug;

const MANIFEST_FILE: &str = ".nutune-manifest.json";

/// Tracks what has been synced to a device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncManifest {
    /// Manifest format version
    pub version: u32,
    /// Last sync timestamp
    pub last_sync: DateTime<Utc>,
    /// Subsonic server URL used for sync
    pub subsonic_url: String,
    /// Albums that have been synced
    pub synced_albums: Vec<SyncedAlbum>,
    /// Playlists that have been synced
    pub synced_playlists: Vec<SyncedPlaylist>,
}

/// Record of a synced album
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncedAlbum {
    /// Subsonic album ID
    pub id: String,
    /// Artist name
    pub artist: String,
    /// Album name
    pub album: String,
    /// Number of tracks synced
    pub track_count: u32,
    /// When this album was synced
    pub synced_at: DateTime<Utc>,
}

/// Record of a synced playlist
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncedPlaylist {
    /// Subsonic playlist ID
    pub id: String,
    /// Playlist name
    pub name: String,
    /// Number of tracks in playlist
    pub track_count: u32,
    /// When this playlist was synced
    pub synced_at: DateTime<Utc>,
}

impl SyncManifest {
    /// Create a new empty manifest
    pub fn new(subsonic_url: &str) -> Self {
        Self {
            version: 1,
            last_sync: Utc::now(),
            subsonic_url: subsonic_url.to_string(),
            synced_albums: Vec::new(),
            synced_playlists: Vec::new(),
        }
    }

    /// Load manifest from device root
    pub fn load(device_root: &Path) -> Result<Option<Self>> {
        let manifest_path = device_root.join(MANIFEST_FILE);

        if !manifest_path.exists() {
            debug!("No manifest found at {}", manifest_path.display());
            return Ok(None);
        }

        let content = std::fs::read_to_string(&manifest_path)
            .context("Failed to read manifest file")?;

        let manifest: Self = serde_json::from_str(&content)
            .context("Failed to parse manifest file")?;

        debug!(
            "Loaded manifest: {} albums, {} playlists",
            manifest.synced_albums.len(),
            manifest.synced_playlists.len()
        );

        Ok(Some(manifest))
    }

    /// Save manifest to device root
    pub fn save(&self, device_root: &Path) -> Result<()> {
        let manifest_path = device_root.join(MANIFEST_FILE);

        let content = serde_json::to_string_pretty(self)
            .context("Failed to serialize manifest")?;

        std::fs::write(&manifest_path, content)
            .context("Failed to write manifest file")?;

        debug!("Saved manifest to {}", manifest_path.display());
        Ok(())
    }

    /// Check if an album has been synced
    pub fn is_album_synced(&self, album_id: &str) -> bool {
        self.synced_albums.iter().any(|a| a.id == album_id)
    }

    /// Check if a playlist has been synced
    pub fn is_playlist_synced(&self, playlist_id: &str) -> bool {
        self.synced_playlists.iter().any(|p| p.id == playlist_id)
    }

    /// Add a synced album
    pub fn add_album(&mut self, album: SyncedAlbum) {
        // Remove existing entry if present (for re-sync)
        self.synced_albums.retain(|a| a.id != album.id);
        self.synced_albums.push(album);
        self.last_sync = Utc::now();
    }

    /// Add a synced playlist
    pub fn add_playlist(&mut self, playlist: SyncedPlaylist) {
        // Remove existing entry if present (for re-sync)
        self.synced_playlists.retain(|p| p.id != playlist.id);
        self.synced_playlists.push(playlist);
        self.last_sync = Utc::now();
    }
}
