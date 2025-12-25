//! Parallel download manager

use anyhow::Result;
use futures::stream::{self, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use std::sync::Arc;
use tracing::debug;

use crate::subsonic::{Song, SubsonicClient};

/// Download task for a single song
#[derive(Debug, Clone)]
pub struct DownloadTask {
    /// Song to download
    pub song: Song,
    /// Destination info (artist, album, track_number for naming)
    pub artist: String,
    pub album: String,
}

/// Result of a download
pub struct DownloadResult {
    pub song: Song,
    pub data: bytes::Bytes,
    pub artist: String,
    pub album: String,
}

/// Parallel downloader with progress tracking
pub struct Downloader {
    client: Arc<SubsonicClient>,
    parallel: usize,
}

impl Downloader {
    /// Create a new downloader
    pub fn new(client: SubsonicClient, parallel: usize) -> Self {
        Self {
            client: Arc::new(client),
            parallel,
        }
    }

    /// Download multiple songs in parallel with progress
    pub async fn download_batch(
        &self,
        tasks: Vec<DownloadTask>,
        progress: &ProgressBar,
    ) -> Result<Vec<DownloadResult>> {
        let total = tasks.len();
        progress.set_length(total as u64);
        progress.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
                .unwrap()
                .progress_chars("#>-"),
        );

        let client = self.client.clone();
        let results: Vec<Result<DownloadResult>> = stream::iter(tasks)
            .map(|task| {
                let client = client.clone();
                async move {
                    let title = task.song.title.clone();
                    debug!("Downloading: {}", title);

                    let data = client.download(&task.song.id).await?;

                    Ok(DownloadResult {
                        song: task.song,
                        data,
                        artist: task.artist,
                        album: task.album,
                    })
                }
            })
            .buffer_unordered(self.parallel)
            .inspect(|result| {
                progress.inc(1);
                if let Ok(r) = result {
                    progress.set_message(format!("{}", r.song.title));
                }
            })
            .collect()
            .await;

        progress.finish_with_message("Downloads complete");

        // Collect successful downloads, log errors
        let mut successful = Vec::new();
        for result in results {
            match result {
                Ok(r) => successful.push(r),
                Err(e) => {
                    tracing::warn!("Download failed: {}", e);
                }
            }
        }

        Ok(successful)
    }

    /// Download a single song
    pub async fn download_one(&self, task: DownloadTask) -> Result<DownloadResult> {
        let data = self.client.download(&task.song.id).await?;

        Ok(DownloadResult {
            song: task.song,
            data,
            artist: task.artist,
            album: task.album,
        })
    }

    /// Download cover art
    pub async fn download_cover_art(&self, id: &str) -> Result<bytes::Bytes> {
        self.client.get_cover_art(id, Some(500)).await
    }
}
