//! Parallel sync pipeline with three stages: download, process, write
//!
//! This module implements a pipelined sync architecture where:
//! - Download stage: Multiple concurrent network downloads
//! - Process stage: Parallel cover art embedding (CPU-bound via spawn_blocking)
//! - Write stage: Sequential writes to device (I/O bound)
//!
//! All stages run concurrently with backpressure via bounded channels.

use anyhow::{Context, Result};
use bytes::Bytes;
use std::sync::Arc;
use tokio::sync::{mpsc, Semaphore};
use tracing::{debug, warn};

use crate::subsonic::Song;

/// Configuration for the sync pipeline
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Number of concurrent downloads (network-bound)
    pub download_parallelism: usize,
    /// Number of concurrent cover art processing tasks (CPU-bound)
    pub processing_parallelism: usize,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            download_parallelism: 4,
            processing_parallelism: 2,
        }
    }
}

/// A track that has been downloaded but not yet processed
#[derive(Debug)]
pub struct DownloadedTrack {
    pub song: Song,
    pub audio_data: Bytes,
    pub artist: String,
    pub album: String,
    pub track_number: u32,
}

/// A track that has been processed (cover art embedded) and is ready to write
#[derive(Debug)]
pub struct ProcessedTrack {
    pub song: Song,
    pub final_audio_data: Vec<u8>,
    pub artist: String,
    pub album: String,
    pub track_number: u32,
}

/// Progress event from the pipeline
#[derive(Debug, Clone)]
pub enum PipelineEvent {
    /// A track was processed (cover embedded)
    Processed,
}

/// Embed cover art into audio data using spawn_blocking (CPU-bound operation)
///
/// This runs the lofty-based embedding in a blocking thread pool to avoid
/// blocking the async runtime.
pub async fn embed_cover_art_async(
    audio_data: Bytes,
    processed_cover: Arc<Vec<u8>>,
    file_extension: String,
) -> Result<Vec<u8>> {
    tokio::task::spawn_blocking(move || {
        embed_cover_art_sync(&audio_data, &processed_cover, &file_extension)
    })
    .await
    .context("Cover art embedding task panicked")?
}

/// Synchronous cover art embedding (called from spawn_blocking)
fn embed_cover_art_sync(
    audio_data: &[u8],
    processed_cover: &[u8],
    file_extension: &str,
) -> Result<Vec<u8>> {
    use lofty::config::WriteOptions;
    use lofty::picture::{MimeType, Picture, PictureType};
    use lofty::prelude::*;
    use lofty::probe::Probe;
    use std::fs;
    use std::io::Write;

    // Create a temp file with the audio data
    // Use a random suffix to ensure uniqueness across threads
    use std::time::{SystemTime, UNIX_EPOCH};
    let random_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!(
        "nutune_embed_{}_{}.{}",
        std::process::id(),
        random_suffix,
        file_extension
    ));

    // Write audio data to temp file
    {
        let mut temp_file =
            fs::File::create(&temp_path).context("Failed to create temp file for cover embedding")?;
        temp_file
            .write_all(audio_data)
            .context("Failed to write audio to temp file")?;
    }

    // Open and modify the temp file
    let mut tagged_file = Probe::open(&temp_path)
        .context("Failed to open temp audio file")?
        .read()
        .context("Failed to read temp audio file")?;

    // Create the picture (cover is already processed)
    let picture = Picture::new_unchecked(
        PictureType::CoverFront,
        Some(MimeType::Jpeg),
        None,
        processed_cover.to_vec(),
    );

    // Get or create tag
    let tag = match tagged_file.primary_tag_mut() {
        Some(tag) => tag,
        None => {
            if let Some(tag) = tagged_file.first_tag_mut() {
                tag
            } else {
                let tag_type = tagged_file.primary_tag_type();
                tagged_file.insert_tag(lofty::tag::Tag::new(tag_type));
                tagged_file
                    .primary_tag_mut()
                    .context("Failed to create tag")?
            }
        }
    };

    // Remove existing cover art and add new one
    tag.remove_picture_type(PictureType::CoverFront);
    tag.push_picture(picture);

    // Save back to the temp file
    tagged_file
        .save_to_path(&temp_path, WriteOptions::default())
        .context("Failed to save audio with embedded cover")?;

    // Read the modified file back
    let result = fs::read(&temp_path).context("Failed to read modified audio file")?;

    // Clean up temp file
    let _ = fs::remove_file(&temp_path);

    Ok(result)
}

/// Process a batch of downloaded tracks with cover art embedding in parallel
///
/// Takes a list of downloaded tracks and a pre-processed cover, and returns
/// processed tracks with cover art embedded.
pub async fn process_tracks_parallel(
    tracks: Vec<DownloadedTrack>,
    processed_cover: Option<Arc<Vec<u8>>>,
    parallelism: usize,
    event_tx: Option<mpsc::Sender<PipelineEvent>>,
) -> Vec<ProcessedTrack> {
    let semaphore = Arc::new(Semaphore::new(parallelism));
    let mut handles = Vec::with_capacity(tracks.len());

    for track in tracks {
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let cover = processed_cover.clone();
        let event_tx = event_tx.clone();

        let handle = tokio::spawn(async move {
            let _permit = permit; // Hold permit until processing completes

            let extension = track
                .song
                .suffix
                .as_deref()
                .unwrap_or("mp3")
                .to_string();
            let title = track.song.title.clone();

            let final_data = if let Some(cover_data) = cover {
                match embed_cover_art_async(track.audio_data.clone(), cover_data, extension).await {
                    Ok(data) => {
                        debug!("Embedded cover art in: {}", title);
                        data
                    }
                    Err(e) => {
                        warn!("Failed to embed cover art in {}: {}", title, e);
                        track.audio_data.to_vec()
                    }
                }
            } else {
                track.audio_data.to_vec()
            };

            if let Some(tx) = event_tx {
                let _ = tx.send(PipelineEvent::Processed).await;
            }

            ProcessedTrack {
                song: track.song,
                final_audio_data: final_data,
                artist: track.artist,
                album: track.album,
                track_number: track.track_number,
            }
        });

        handles.push(handle);
    }

    // Collect results, preserving order for track numbers
    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        match handle.await {
            Ok(processed) => results.push(processed),
            Err(e) => {
                warn!("Processing task panicked: {}", e);
            }
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_config_default() {
        let config = PipelineConfig::default();
        assert_eq!(config.download_parallelism, 4);
        assert_eq!(config.processing_parallelism, 2);
    }
}
