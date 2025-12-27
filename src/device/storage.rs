//! Device storage operations

use anyhow::{Context, Result};
use std::path::PathBuf;
use tokio::fs;
use tracing::debug;

use crate::utils::sanitize_filename;

/// Manages file operations on a device
pub struct DeviceStorage {
    root: PathBuf,
}

impl DeviceStorage {
    /// Create a new storage manager for a device
    pub fn new(mount_point: PathBuf) -> Self {
        Self { root: mount_point }
    }

    /// Get path to Artists directory
    pub fn artists_dir(&self) -> PathBuf {
        self.root.join("Artists")
    }

    /// Get path to Playlists directory
    pub fn playlists_dir(&self) -> PathBuf {
        self.root.join("Playlists")
    }

    /// Create the base directory structure
    pub async fn init(&self) -> Result<()> {
        fs::create_dir_all(self.artists_dir())
            .await
            .context("Failed to create Artists directory")?;

        fs::create_dir_all(self.playlists_dir())
            .await
            .context("Failed to create Playlists directory")?;

        debug!("Initialized directory structure at {}", self.root.display());
        Ok(())
    }

    /// Create artist/album folder structure and return the album path
    pub async fn create_album_folder(&self, artist: &str, album: &str) -> Result<PathBuf> {
        let artist_safe = sanitize_filename(artist);
        let album_safe = sanitize_filename(album);

        let album_path = self.artists_dir().join(&artist_safe).join(&album_safe);

        fs::create_dir_all(&album_path)
            .await
            .context("Failed to create album directory")?;

        debug!("Created album folder: {}", album_path.display());
        Ok(album_path)
    }

    /// Create playlist folder and return the path
    pub async fn create_playlist_folder(&self, name: &str) -> Result<PathBuf> {
        let name_safe = sanitize_filename(name);
        let playlist_path = self.playlists_dir().join(&name_safe);

        fs::create_dir_all(&playlist_path)
            .await
            .context("Failed to create playlist directory")?;

        debug!("Created playlist folder: {}", playlist_path.display());
        Ok(playlist_path)
    }

    /// Write a track file to an album folder
    ///
    /// Returns the full path of the written file
    pub async fn write_album_track(
        &self,
        artist: &str,
        album: &str,
        track_number: u32,
        title: &str,
        extension: &str,
        data: &[u8],
    ) -> Result<PathBuf> {
        let album_path = self.create_album_folder(artist, album).await?;

        let title_safe = sanitize_filename(title);
        let filename = format!("{:02} - {}.{}", track_number, title_safe, extension);
        let file_path = album_path.join(&filename);

        fs::write(&file_path, data)
            .await
            .context("Failed to write track file")?;

        debug!("Wrote track: {}", file_path.display());
        Ok(file_path)
    }

    /// Write a track file to a playlist folder
    ///
    /// Returns the filename (not full path) for use in M3U
    pub async fn write_playlist_track(
        &self,
        playlist_name: &str,
        artist: &str,
        title: &str,
        extension: &str,
        data: &[u8],
    ) -> Result<String> {
        let playlist_path = self.create_playlist_folder(playlist_name).await?;

        let artist_safe = sanitize_filename(artist);
        let title_safe = sanitize_filename(title);
        let filename = format!("{} - {}.{}", artist_safe, title_safe, extension);
        let file_path = playlist_path.join(&filename);

        fs::write(&file_path, data)
            .await
            .context("Failed to write playlist track")?;

        debug!("Wrote playlist track: {}", file_path.display());
        Ok(filename)
    }

    /// Write cover art to an album folder
    pub async fn write_cover_art(
        &self,
        artist: &str,
        album: &str,
        data: &[u8],
    ) -> Result<PathBuf> {
        let album_path = self.create_album_folder(artist, album).await?;
        let cover_path = album_path.join("cover.jpg");

        fs::write(&cover_path, data)
            .await
            .context("Failed to write cover art")?;

        debug!("Wrote cover art: {}", cover_path.display());
        Ok(cover_path)
    }

    /// Generate and write an M3U playlist file
    pub async fn write_m3u(&self, playlist_name: &str, tracks: &[String]) -> Result<PathBuf> {
        let playlist_path = self.create_playlist_folder(playlist_name).await?;
        let m3u_path = playlist_path.join("playlist.m3u");

        let content = crate::utils::generate_m3u(tracks);

        fs::write(&m3u_path, content)
            .await
            .context("Failed to write M3U file")?;

        debug!("Wrote M3U: {} ({} tracks)", m3u_path.display(), tracks.len());
        Ok(m3u_path)
    }
}
