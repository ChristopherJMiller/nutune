//! Sync engine orchestration

use anyhow::Result;
use bytes::Bytes;
use chrono::Utc;
use futures::stream::{self, StreamExt};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::device::{DeviceStorage, SyncManifest, SyncedAlbum, SyncedPlaylist};
use crate::subsonic::{Album, Playlist, SubsonicClient, SyncSelection};
use crate::sync::downloader::{DownloadTask, DownloadResult, Downloader};
use crate::sync::pipeline::{DownloadedTrack, PipelineConfig, process_tracks_parallel};
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
        albums_deleted: usize,
        playlists_deleted: usize,
    },
    /// Deletion phase starting
    DeletionStarted {
        albums_to_delete: usize,
        playlists_to_delete: usize,
    },
    /// An album was deleted
    AlbumDeleted {
        artist: String,
        album: String,
    },
    /// Album deletion failed
    AlbumDeleteFailed {
        artist: String,
        album: String,
        error: String,
    },
    /// A playlist was deleted
    PlaylistDeleted {
        name: String,
    },
    /// Playlist deletion failed
    PlaylistDeleteFailed {
        name: String,
        error: String,
    },
}

/// Items to be deleted from device
#[derive(Debug, Clone, Default)]
pub struct DeletionSelection {
    /// Album IDs to delete (id, artist, album_name)
    pub albums: Vec<(String, String, String)>,
    /// Playlist IDs to delete (id, name)
    pub playlists: Vec<(String, String)>,
}

impl DeletionSelection {
    pub fn is_empty(&self) -> bool {
        self.albums.is_empty() && self.playlists.is_empty()
    }
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
    pipeline_config: PipelineConfig,
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

        // Configure pipeline with download parallelism from param, processing at half
        let pipeline_config = PipelineConfig {
            download_parallelism: parallel,
            processing_parallelism: (parallel / 2).max(1),
        };

        Ok(Self {
            client,
            storage,
            manifest,
            downloader,
            device_path,
            pipeline_config,
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

    /// Delete items that are no longer selected
    pub async fn delete_deselected(
        &mut self,
        deletions: &DeletionSelection,
        progress_tx: &mpsc::Sender<SyncProgress>,
    ) -> Result<(usize, usize)> {
        let mut albums_deleted = 0;
        let mut playlists_deleted = 0;

        if deletions.is_empty() {
            return Ok((0, 0));
        }

        // Send start event
        let _ = progress_tx.send(SyncProgress::DeletionStarted {
            albums_to_delete: deletions.albums.len(),
            playlists_to_delete: deletions.playlists.len(),
        }).await;

        // Delete albums
        for (album_id, artist, album) in &deletions.albums {
            match self.storage.delete_album(artist, album).await {
                Ok(()) => {
                    self.manifest.remove_album(album_id);
                    albums_deleted += 1;
                    let _ = progress_tx.send(SyncProgress::AlbumDeleted {
                        artist: artist.clone(),
                        album: album.clone(),
                    }).await;
                }
                Err(e) => {
                    let _ = progress_tx.send(SyncProgress::AlbumDeleteFailed {
                        artist: artist.clone(),
                        album: album.clone(),
                        error: e.to_string(),
                    }).await;
                }
            }
        }

        // Delete playlists
        for (playlist_id, name) in &deletions.playlists {
            match self.storage.delete_playlist(name).await {
                Ok(()) => {
                    self.manifest.remove_playlist(playlist_id);
                    playlists_deleted += 1;
                    let _ = progress_tx.send(SyncProgress::PlaylistDeleted {
                        name: name.clone(),
                    }).await;
                }
                Err(e) => {
                    let _ = progress_tx.send(SyncProgress::PlaylistDeleteFailed {
                        name: name.clone(),
                        error: e.to_string(),
                    }).await;
                }
            }
        }

        Ok((albums_deleted, playlists_deleted))
    }

    /// Execute sync with progress updates sent to a channel (for TUI)
    pub async fn sync_with_progress(
        &mut self,
        selection: &SyncSelection,
        deletions: &DeletionSelection,
        progress_tx: mpsc::Sender<SyncProgress>,
    ) -> Result<SyncResult> {
        let mut result = SyncResult::default();

        // Initialize storage directories
        self.storage.init().await?;

        // Phase 1: Delete deselected items first
        let (albums_deleted, playlists_deleted) = self.delete_deselected(deletions, &progress_tx).await?;

        // Send start event for downloads
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
            albums_deleted,
            playlists_deleted,
        }).await;

        Ok(result)
    }

    /// Sync a single album with progress reporting (pipelined parallel version)
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

        // Download and process cover art first (cached for all tracks)
        let processed_cover: Option<Arc<Vec<u8>>> = if let Some(cover_id) = &album.cover_art {
            match self.downloader.download_cover_art(cover_id).await {
                Ok(data) => {
                    // Process cover art once and cache it
                    match cover_art::process_cover_art(&data) {
                        Ok(processed) => Some(Arc::new(processed)),
                        Err(e) => {
                            warn!("Failed to process cover art: {}", e);
                            None
                        }
                    }
                }
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
        let _ = progress_tx
            .send(SyncProgress::AlbumStarted {
                artist: artist.to_string(),
                album: album.name.clone(),
                track_count,
            })
            .await;

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

        // Stage 1: Download all tracks in parallel
        let client = self.downloader.client_arc();
        let parallelism = self.pipeline_config.download_parallelism;
        let progress_tx_clone = progress_tx.clone();

        let downloads: Vec<DownloadResult> = stream::iter(tasks)
            .map(|task| {
                let client = client.clone();
                async move {
                    let data = client.download(&task.song.id).await?;
                    Ok::<_, anyhow::Error>(DownloadResult {
                        song: task.song,
                        data,
                        artist: task.artist,
                        album: task.album,
                    })
                }
            })
            .buffer_unordered(parallelism)
            .filter_map(|result| async {
                match result {
                    Ok(r) => Some(r),
                    Err(e) => {
                        warn!("Download failed: {}", e);
                        None
                    }
                }
            })
            .collect()
            .await;

        // Send progress event for downloads completion
        let _ = progress_tx_clone
            .send(SyncProgress::TrackCompleted {
                track_num: downloads.len(),
                total_tracks: track_count,
            })
            .await;

        // Stage 2: Convert to DownloadedTrack for pipeline processing
        let downloaded_tracks: Vec<DownloadedTrack> = downloads
            .into_iter()
            .map(|dl| DownloadedTrack {
                song: dl.song.clone(),
                audio_data: dl.data,
                artist: dl.artist,
                album: dl.album,
                track_number: dl.song.track.unwrap_or(1),
            })
            .collect();

        // Stage 3: Process cover art embedding in parallel
        let processed_tracks = process_tracks_parallel(
            downloaded_tracks,
            processed_cover.clone(),
            self.pipeline_config.processing_parallelism,
            None, // Events handled at album level
        )
        .await;

        // Stage 4: Write tracks to device
        let mut total_bytes: u64 = 0;
        for track in &processed_tracks {
            let extension = track.song.suffix.as_deref().unwrap_or("mp3");

            total_bytes += track.final_audio_data.len() as u64;

            self.storage
                .write_album_track(
                    &track.artist,
                    &track.album,
                    track.track_number,
                    &track.song.title,
                    extension,
                    &track.final_audio_data,
                )
                .await?;
        }

        // Also save cover art as file (for file browsers/fallback)
        if let Some(ref cover) = processed_cover
            && let Err(e) = self
                .storage
                .write_cover_art(artist, &album.name, cover)
                .await
            {
                debug!("Failed to write cover.jpg: {}", e);
            }

        // Update manifest
        self.manifest.add_album(SyncedAlbum {
            id: album.id.clone(),
            artist: artist.to_string(),
            album: album.name.clone(),
            track_count: processed_tracks.len() as u32,
            synced_at: Utc::now(),
        });

        Ok((processed_tracks.len(), total_bytes))
    }

    /// Sync a single playlist with progress reporting (pipelined parallel version)
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
        let _ = progress_tx
            .send(SyncProgress::PlaylistStarted {
                name: playlist.name.clone(),
                track_count,
            })
            .await;

        // Create download tasks with cover art IDs
        let tasks_with_covers: Vec<(DownloadTask, Option<String>)> = playlist_details
            .songs
            .iter()
            .map(|song| {
                let task = DownloadTask {
                    song: song.clone(),
                    artist: song
                        .artist
                        .clone()
                        .unwrap_or_else(|| "Unknown Artist".to_string()),
                    album: playlist.name.clone(),
                };
                let cover_id = song.cover_art.clone();
                (task, cover_id)
            })
            .collect();

        // Stage 1: Download all tracks and their covers in parallel
        let client = self.downloader.client_arc();
        let parallelism = self.pipeline_config.download_parallelism;

        // Download struct to hold track + its cover
        struct PlaylistDownload {
            download: DownloadResult,
            cover_data: Option<Bytes>,
            cover_id: Option<String>,
        }

        let downloads: Vec<PlaylistDownload> = stream::iter(tasks_with_covers)
            .map(|(task, cover_id)| {
                let client = client.clone();
                let cover_id_clone = cover_id.clone();
                async move {
                    // Download the track
                    let data = client.download(&task.song.id).await?;
                    let download = DownloadResult {
                        song: task.song,
                        data,
                        artist: task.artist,
                        album: task.album,
                    };

                    // Download cover art if available
                    let cover_data = if let Some(ref cid) = cover_id_clone {
                        match client.get_cover_art(cid, Some(500)).await {
                            Ok(data) => Some(data),
                            Err(e) => {
                                debug!("Failed to download cover for playlist track: {}", e);
                                None
                            }
                        }
                    } else {
                        None
                    };

                    Ok::<_, anyhow::Error>(PlaylistDownload {
                        download,
                        cover_data,
                        cover_id: cover_id_clone,
                    })
                }
            })
            .buffer_unordered(parallelism)
            .filter_map(|result| async {
                match result {
                    Ok(r) => Some(r),
                    Err(e) => {
                        warn!("Download failed: {}", e);
                        None
                    }
                }
            })
            .collect()
            .await;

        // Send progress event for downloads completion
        let _ = progress_tx
            .send(SyncProgress::TrackCompleted {
                track_num: downloads.len(),
                total_tracks: track_count,
            })
            .await;

        // Stage 2: Process covers and embed in parallel
        // Use a cache to avoid reprocessing the same cover for different tracks
        let mut cover_cache: std::collections::HashMap<String, Arc<Vec<u8>>> =
            std::collections::HashMap::new();

        // Pre-process unique covers
        for dl in &downloads {
            if let (Some(cover_id), Some(cover_data)) = (&dl.cover_id, &dl.cover_data)
                && !cover_cache.contains_key(cover_id) {
                    match cover_art::process_cover_art(cover_data) {
                        Ok(processed) => {
                            cover_cache.insert(cover_id.clone(), Arc::new(processed));
                        }
                        Err(e) => {
                            warn!("Failed to process cover {}: {}", cover_id, e);
                        }
                    }
                }
        }

        // Stage 3: Embed covers in parallel using spawn_blocking
        use crate::sync::pipeline::embed_cover_art_async;
        use tokio::sync::Semaphore;

        let semaphore = Arc::new(Semaphore::new(self.pipeline_config.processing_parallelism));
        let mut embed_handles = Vec::with_capacity(downloads.len());

        for dl in downloads {
            let processed_cover = dl
                .cover_id
                .as_ref()
                .and_then(|id| cover_cache.get(id).cloned());

            let permit = semaphore.clone().acquire_owned().await.unwrap();
            let extension = dl
                .download
                .song
                .suffix
                .clone()
                .unwrap_or_else(|| "mp3".to_string());
            let audio_data = dl.download.data.clone();
            let song = dl.download.song.clone();
            let artist = dl.download.artist.clone();

            let handle = tokio::spawn(async move {
                let _permit = permit;

                let final_data = if let Some(cover) = processed_cover {
                    match embed_cover_art_async(audio_data.clone(), cover, extension.clone()).await
                    {
                        Ok(data) => data,
                        Err(e) => {
                            warn!("Failed to embed cover in {}: {}", song.title, e);
                            audio_data.to_vec()
                        }
                    }
                } else {
                    audio_data.to_vec()
                };

                (song, artist, extension, final_data)
            });

            embed_handles.push(handle);
        }

        // Collect processed tracks
        let mut processed_tracks = Vec::with_capacity(embed_handles.len());
        for handle in embed_handles {
            match handle.await {
                Ok(result) => processed_tracks.push(result),
                Err(e) => {
                    warn!("Embed task panicked: {}", e);
                }
            }
        }

        // Stage 4: Write tracks to device
        let mut total_bytes: u64 = 0;
        let mut track_filenames: Vec<String> = Vec::new();

        for (song, artist, extension, final_data) in &processed_tracks {
            total_bytes += final_data.len() as u64;

            let filename = self
                .storage
                .write_playlist_track(
                    &playlist.name,
                    artist,
                    &song.title,
                    extension,
                    final_data,
                )
                .await?;

            track_filenames.push(filename);
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
        if let Some(ref cover) = cover_data
            && let Err(e) = self.storage.write_cover_art(artist, &album.name, cover).await {
                debug!("Failed to write cover.jpg: {}", e);
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

