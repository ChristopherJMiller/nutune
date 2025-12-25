//! Sync engine orchestration

use anyhow::Result;
use chrono::Utc;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::path::PathBuf;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::device::{DeviceStorage, SyncManifest, SyncedAlbum, SyncedPlaylist};
use crate::subsonic::{Album, Playlist, SubsonicClient, SyncSelection};
use crate::sync::downloader::{DownloadTask, DownloadResult, Downloader};
use crate::utils::cover_art;

/// Progress updates sent during sync
#[derive(Debug, Clone)]
pub enum SyncProgress {
    /// Starting sync
    Started {
        total_albums: usize,
        total_playlists: usize,
    },
    /// Starting an album
    AlbumStarted {
        artist: String,
        album: String,
        track_count: usize,
    },
    /// A track was downloaded
    TrackCompleted {
        track_num: usize,
        total_tracks: usize,
    },
    /// An album finished
    AlbumCompleted {
        artist: String,
        album: String,
    },
    /// An album was skipped (already synced)
    AlbumSkipped {
        artist: String,
        album: String,
    },
    /// Starting a playlist
    PlaylistStarted {
        name: String,
        track_count: usize,
    },
    /// A playlist finished
    PlaylistCompleted {
        name: String,
    },
    /// A playlist was skipped (already synced)
    PlaylistSkipped {
        name: String,
    },
    /// Error occurred
    Error {
        message: String,
    },
    /// Sync complete
    Complete {
        albums_synced: usize,
        playlists_synced: usize,
        tracks_downloaded: usize,
        bytes_downloaded: u64,
    },
}

/// Result of a sync operation
#[derive(Debug, Default)]
pub struct SyncResult {
    pub albums_synced: usize,
    pub playlists_synced: usize,
    pub tracks_downloaded: usize,
    pub bytes_downloaded: u64,
}

/// Sync engine that coordinates downloading and writing to device
pub struct SyncEngine {
    client: SubsonicClient,
    storage: DeviceStorage,
    manifest: SyncManifest,
    downloader: Downloader,
    device_path: PathBuf,
}

impl SyncEngine {
    /// Create a new sync engine
    pub fn new(client: SubsonicClient, device_path: PathBuf, parallel: usize) -> Result<Self> {
        let storage = DeviceStorage::new(device_path.clone());

        // Load or create manifest
        let manifest = SyncManifest::load(&device_path)?
            .unwrap_or_else(|| {
                // Create new manifest - we'll get the URL later
                SyncManifest::new("unknown")
            });

        let downloader = Downloader::new(client.clone(), parallel);

        Ok(Self {
            client,
            storage,
            manifest,
            downloader,
            device_path,
        })
    }

    /// Execute sync based on selection
    pub async fn sync(&mut self, selection: &SyncSelection) -> Result<SyncResult> {
        let mut result = SyncResult::default();

        // Initialize storage directories
        self.storage.init().await?;

        // Set up progress display
        let multi = MultiProgress::new();

        // Sync albums
        for album in &selection.albums {
            let spinner = multi.add(ProgressBar::new_spinner());
            spinner.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.green} {msg}")
                    .unwrap(),
            );

            match self.sync_album(album, &multi).await {
                Ok((tracks, bytes)) => {
                    result.albums_synced += 1;
                    result.tracks_downloaded += tracks;
                    result.bytes_downloaded += bytes;
                    spinner.finish_with_message(format!(
                        "Album synced: {} - {}",
                        album.artist.as_deref().unwrap_or("Unknown"),
                        album.name
                    ));
                }
                Err(e) => {
                    spinner.finish_with_message(format!("Failed: {} - {}", album.name, e));
                    tracing::error!("Failed to sync album {}: {}", album.name, e);
                }
            }
        }

        // Sync playlists
        for playlist in &selection.playlists {
            let spinner = multi.add(ProgressBar::new_spinner());
            spinner.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.green} {msg}")
                    .unwrap(),
            );

            match self.sync_playlist(playlist, &multi).await {
                Ok((tracks, bytes)) => {
                    result.playlists_synced += 1;
                    result.tracks_downloaded += tracks;
                    result.bytes_downloaded += bytes;
                    spinner.finish_with_message(format!("Playlist synced: {}", playlist.name));
                }
                Err(e) => {
                    spinner.finish_with_message(format!("Failed: {} - {}", playlist.name, e));
                    tracing::error!("Failed to sync playlist {}: {}", playlist.name, e);
                }
            }
        }

        // Save manifest
        self.manifest.save(&self.device_path)?;

        Ok(result)
    }

    /// Execute sync with progress updates sent to a channel (for TUI)
    pub async fn sync_with_progress(
        &mut self,
        selection: &SyncSelection,
        progress_tx: mpsc::Sender<SyncProgress>,
    ) -> Result<SyncResult> {
        let mut result = SyncResult::default();

        // Initialize storage directories
        self.storage.init().await?;

        // Send start event
        let _ = progress_tx.send(SyncProgress::Started {
            total_albums: selection.albums.len(),
            total_playlists: selection.playlists.len(),
        }).await;

        // Sync albums
        for album in &selection.albums {
            let artist = album.artist.as_deref().unwrap_or("Unknown Artist").to_string();

            match self.sync_album_with_progress(album, &progress_tx).await {
                Ok((tracks, bytes)) => {
                    if tracks > 0 {
                        result.albums_synced += 1;
                        result.tracks_downloaded += tracks;
                        result.bytes_downloaded += bytes;
                        let _ = progress_tx.send(SyncProgress::AlbumCompleted {
                            artist: artist.clone(),
                            album: album.name.clone(),
                        }).await;
                    } else {
                        let _ = progress_tx.send(SyncProgress::AlbumSkipped {
                            artist: artist.clone(),
                            album: album.name.clone(),
                        }).await;
                    }
                }
                Err(e) => {
                    let _ = progress_tx.send(SyncProgress::Error {
                        message: format!("Album {} - {}: {}", artist, album.name, e),
                    }).await;
                    tracing::error!("Failed to sync album {}: {}", album.name, e);
                }
            }
        }

        // Sync playlists
        for playlist in &selection.playlists {
            match self.sync_playlist_with_progress(playlist, &progress_tx).await {
                Ok((tracks, bytes)) => {
                    if tracks > 0 {
                        result.playlists_synced += 1;
                        result.tracks_downloaded += tracks;
                        result.bytes_downloaded += bytes;
                        let _ = progress_tx.send(SyncProgress::PlaylistCompleted {
                            name: playlist.name.clone(),
                        }).await;
                    } else {
                        let _ = progress_tx.send(SyncProgress::PlaylistSkipped {
                            name: playlist.name.clone(),
                        }).await;
                    }
                }
                Err(e) => {
                    let _ = progress_tx.send(SyncProgress::Error {
                        message: format!("Playlist {}: {}", playlist.name, e),
                    }).await;
                    tracing::error!("Failed to sync playlist {}: {}", playlist.name, e);
                }
            }
        }

        // Save manifest
        self.manifest.save(&self.device_path)?;

        // Send completion event
        let _ = progress_tx.send(SyncProgress::Complete {
            albums_synced: result.albums_synced,
            playlists_synced: result.playlists_synced,
            tracks_downloaded: result.tracks_downloaded,
            bytes_downloaded: result.bytes_downloaded,
        }).await;

        Ok(result)
    }

    /// Sync a single album with progress reporting
    async fn sync_album_with_progress(
        &mut self,
        album: &Album,
        progress_tx: &mpsc::Sender<SyncProgress>,
    ) -> Result<(usize, u64)> {
        let artist = album.artist.as_deref().unwrap_or("Unknown Artist");

        // Check if already synced
        if self.manifest.is_album_synced(&album.id) {
            debug!("Album already synced: {} - {}", artist, album.name);
            return Ok((0, 0));
        }

        info!("Syncing album: {} - {}", artist, album.name);

        // Download cover art first (needed for embedding)
        let cover_data = if let Some(cover_id) = &album.cover_art {
            match self.downloader.download_cover_art(cover_id).await {
                Ok(data) => Some(data),
                Err(e) => {
                    warn!("Failed to download cover art: {}", e);
                    None
                }
            }
        } else {
            None
        };

        // Fetch album details with songs
        let album_details = self.client.get_album(&album.id).await?;
        let track_count = album_details.song.len();

        // Send start event
        let _ = progress_tx.send(SyncProgress::AlbumStarted {
            artist: artist.to_string(),
            album: album.name.clone(),
            track_count,
        }).await;

        // Create download tasks
        let tasks: Vec<DownloadTask> = album_details
            .song
            .iter()
            .map(|song| DownloadTask {
                song: song.clone(),
                artist: artist.to_string(),
                album: album.name.clone(),
            })
            .collect();

        // Download tracks one by one with progress updates
        let mut downloads = Vec::new();
        for (idx, task) in tasks.into_iter().enumerate() {
            let download = self.downloader.download_one(task).await?;
            downloads.push(download);

            let _ = progress_tx.send(SyncProgress::TrackCompleted {
                track_num: idx + 1,
                total_tracks: track_count,
            }).await;
        }

        let mut total_bytes: u64 = 0;

        // Write tracks to device with embedded cover art
        for download in &downloads {
            let track_num = download.song.track.unwrap_or(1);
            let extension = download.song.suffix.as_deref().unwrap_or("mp3");

            // Embed cover art if available
            let audio_data = if let Some(ref cover) = cover_data {
                match cover_art::embed_cover_art_in_memory(&download.data, cover, extension) {
                    Ok(data) => {
                        debug!("Embedded cover art in: {}", download.song.title);
                        data.into()
                    }
                    Err(e) => {
                        warn!("Failed to embed cover art in {}: {}", download.song.title, e);
                        download.data.clone()
                    }
                }
            } else {
                download.data.clone()
            };

            total_bytes += audio_data.len() as u64;

            self.storage
                .write_album_track(
                    &download.artist,
                    &download.album,
                    track_num,
                    &download.song.title,
                    extension,
                    &audio_data,
                )
                .await?;
        }

        // Also save cover art as file (for file browsers/fallback)
        if let Some(ref cover) = cover_data {
            if let Err(e) = self.storage.write_cover_art(artist, &album.name, cover).await {
                debug!("Failed to write cover.jpg: {}", e);
            }
        }

        // Update manifest
        self.manifest.add_album(SyncedAlbum {
            id: album.id.clone(),
            artist: artist.to_string(),
            album: album.name.clone(),
            track_count: downloads.len() as u32,
            synced_at: Utc::now(),
        });

        Ok((downloads.len(), total_bytes))
    }

    /// Sync a single playlist with progress reporting
    async fn sync_playlist_with_progress(
        &mut self,
        playlist: &Playlist,
        progress_tx: &mpsc::Sender<SyncProgress>,
    ) -> Result<(usize, u64)> {
        // Check if already synced
        if self.manifest.is_playlist_synced(&playlist.id) {
            debug!("Playlist already synced: {}", playlist.name);
            return Ok((0, 0));
        }

        info!("Syncing playlist: {}", playlist.name);

        // Fetch playlist details with songs
        let playlist_details = self.client.get_playlist(&playlist.id).await?;
        let track_count = playlist_details.songs.len();

        // Send start event
        let _ = progress_tx.send(SyncProgress::PlaylistStarted {
            name: playlist.name.clone(),
            track_count,
        }).await;

        // Create download tasks with cover art IDs
        let tasks_with_covers: Vec<(DownloadTask, Option<String>)> = playlist_details
            .songs
            .iter()
            .map(|song| {
                let task = DownloadTask {
                    song: song.clone(),
                    artist: song.artist.clone().unwrap_or_else(|| "Unknown Artist".to_string()),
                    album: playlist.name.clone(),
                };
                let cover_id = song.cover_art.clone();
                (task, cover_id)
            })
            .collect();

        let mut total_bytes: u64 = 0;
        let mut track_filenames: Vec<String> = Vec::new();

        // Download and write tracks one by one with progress updates
        for (idx, (task, cover_id)) in tasks_with_covers.into_iter().enumerate() {
            let download = self.downloader.download_one(task).await?;

            // Download cover art for this track
            let cover_data = if let Some(ref cid) = cover_id {
                match self.downloader.download_cover_art(cid).await {
                    Ok(data) => Some(data),
                    Err(e) => {
                        debug!("Failed to download cover for playlist track: {}", e);
                        None
                    }
                }
            } else {
                None
            };

            let extension = download.song.suffix.as_deref().unwrap_or("mp3");
            let artist = download.song.artist.as_deref().unwrap_or("Unknown Artist");

            // Embed cover art if available
            let audio_data = if let Some(ref cover) = cover_data {
                match cover_art::embed_cover_art_in_memory(&download.data, cover, extension) {
                    Ok(data) => {
                        debug!("Embedded cover art in playlist track: {}", download.song.title);
                        data.into()
                    }
                    Err(e) => {
                        warn!("Failed to embed cover art in {}: {}", download.song.title, e);
                        download.data.clone()
                    }
                }
            } else {
                download.data.clone()
            };

            total_bytes += audio_data.len() as u64;

            let filename = self
                .storage
                .write_playlist_track(
                    &playlist.name,
                    artist,
                    &download.song.title,
                    extension,
                    &audio_data,
                )
                .await?;

            track_filenames.push(filename);

            let _ = progress_tx.send(SyncProgress::TrackCompleted {
                track_num: idx + 1,
                total_tracks: track_count,
            }).await;
        }

        // Write M3U playlist file
        self.storage
            .write_m3u(&playlist.name, &track_filenames)
            .await?;

        // Update manifest
        self.manifest.add_playlist(SyncedPlaylist {
            id: playlist.id.clone(),
            name: playlist.name.clone(),
            track_count: track_filenames.len() as u32,
            synced_at: Utc::now(),
        });

        Ok((track_filenames.len(), total_bytes))
    }

    /// Sync a single album
    async fn sync_album(
        &mut self,
        album: &Album,
        multi: &MultiProgress,
    ) -> Result<(usize, u64)> {
        let artist = album.artist.as_deref().unwrap_or("Unknown Artist");

        // Check if already synced
        if self.manifest.is_album_synced(&album.id) {
            debug!("Album already synced: {} - {}", artist, album.name);
            return Ok((0, 0));
        }

        info!("Syncing album: {} - {}", artist, album.name);

        // Download cover art first (needed for embedding)
        let cover_data = if let Some(cover_id) = &album.cover_art {
            match self.downloader.download_cover_art(cover_id).await {
                Ok(data) => Some(data),
                Err(e) => {
                    warn!("Failed to download cover art: {}", e);
                    None
                }
            }
        } else {
            None
        };

        // Fetch album details with songs
        let album_details = self.client.get_album(&album.id).await?;

        // Create download tasks
        let tasks: Vec<DownloadTask> = album_details
            .song
            .iter()
            .map(|song| DownloadTask {
                song: song.clone(),
                artist: artist.to_string(),
                album: album.name.clone(),
            })
            .collect();

        let task_count = tasks.len();

        // Download tracks
        let progress = multi.add(ProgressBar::new(task_count as u64));
        let downloads = self.downloader.download_batch(tasks, &progress).await?;

        let mut total_bytes: u64 = 0;

        // Write tracks to device with embedded cover art
        for download in &downloads {
            let track_num = download.song.track.unwrap_or(1);
            let extension = download.song.suffix.as_deref().unwrap_or("mp3");

            // Embed cover art if available
            let audio_data = if let Some(ref cover) = cover_data {
                match cover_art::embed_cover_art_in_memory(&download.data, cover, extension) {
                    Ok(data) => {
                        debug!("Embedded cover art in: {}", download.song.title);
                        data.into()
                    }
                    Err(e) => {
                        warn!("Failed to embed cover art in {}: {}", download.song.title, e);
                        download.data.clone()
                    }
                }
            } else {
                download.data.clone()
            };

            total_bytes += audio_data.len() as u64;

            self.storage
                .write_album_track(
                    &download.artist,
                    &download.album,
                    track_num,
                    &download.song.title,
                    extension,
                    &audio_data,
                )
                .await?;
        }

        // Also save cover art as file (for file browsers/fallback)
        if let Some(ref cover) = cover_data {
            if let Err(e) = self.storage.write_cover_art(artist, &album.name, cover).await {
                debug!("Failed to write cover.jpg: {}", e);
            }
        }

        // Update manifest
        self.manifest.add_album(SyncedAlbum {
            id: album.id.clone(),
            artist: artist.to_string(),
            album: album.name.clone(),
            track_count: downloads.len() as u32,
            synced_at: Utc::now(),
        });

        Ok((downloads.len(), total_bytes))
    }

    /// Sync a single playlist
    async fn sync_playlist(
        &mut self,
        playlist: &Playlist,
        multi: &MultiProgress,
    ) -> Result<(usize, u64)> {
        // Check if already synced
        if self.manifest.is_playlist_synced(&playlist.id) {
            debug!("Playlist already synced: {}", playlist.name);
            return Ok((0, 0));
        }

        info!("Syncing playlist: {}", playlist.name);

        // Fetch playlist details with songs
        let playlist_details = self.client.get_playlist(&playlist.id).await?;
        let track_count = playlist_details.songs.len();

        // Create download tasks with cover art IDs
        let tasks_with_covers: Vec<(DownloadTask, Option<String>)> = playlist_details
            .songs
            .iter()
            .map(|song| {
                let task = DownloadTask {
                    song: song.clone(),
                    artist: song.artist.clone().unwrap_or_else(|| "Unknown Artist".to_string()),
                    album: playlist.name.clone(),
                };
                let cover_id = song.cover_art.clone();
                (task, cover_id)
            })
            .collect();

        let progress = multi.add(ProgressBar::new(track_count as u64));
        progress.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
                .unwrap()
                .progress_chars("#>-"),
        );

        let mut total_bytes: u64 = 0;
        let mut track_filenames: Vec<String> = Vec::new();

        // Download and write tracks one by one (to embed cover art per track)
        for (task, cover_id) in tasks_with_covers {
            let download = self.downloader.download_one(task).await?;

            // Download cover art for this track
            let cover_data = if let Some(ref cid) = cover_id {
                match self.downloader.download_cover_art(cid).await {
                    Ok(data) => Some(data),
                    Err(e) => {
                        debug!("Failed to download cover for playlist track: {}", e);
                        None
                    }
                }
            } else {
                None
            };

            let extension = download.song.suffix.as_deref().unwrap_or("mp3");
            let artist = download.song.artist.as_deref().unwrap_or("Unknown Artist");

            // Embed cover art if available
            let audio_data = if let Some(ref cover) = cover_data {
                match cover_art::embed_cover_art_in_memory(&download.data, cover, extension) {
                    Ok(data) => {
                        debug!("Embedded cover art in playlist track: {}", download.song.title);
                        data.into()
                    }
                    Err(e) => {
                        warn!("Failed to embed cover art in {}: {}", download.song.title, e);
                        download.data.clone()
                    }
                }
            } else {
                download.data.clone()
            };

            total_bytes += audio_data.len() as u64;

            let filename = self
                .storage
                .write_playlist_track(
                    &playlist.name,
                    artist,
                    &download.song.title,
                    extension,
                    &audio_data,
                )
                .await?;

            track_filenames.push(filename);
            progress.inc(1);
            progress.set_message(download.song.title.clone());
        }

        progress.finish_with_message("Done");

        // Write M3U playlist file
        self.storage
            .write_m3u(&playlist.name, &track_filenames)
            .await?;

        // Update manifest
        self.manifest.add_playlist(SyncedPlaylist {
            id: playlist.id.clone(),
            name: playlist.name.clone(),
            track_count: track_filenames.len() as u32,
            synced_at: Utc::now(),
        });

        Ok((track_filenames.len(), total_bytes))
    }
}

