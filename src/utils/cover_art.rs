//! Cover art processing and embedding for portable device compatibility
//!
//! Optimized for FiiO Snowsky Echo Mini requirements:
//! - JPEG format with baseline encoding
//! - Max 300x300 pixels (maximum compatibility)
//! - Under 200KB file size
//! - Embedded in audio file metadata

use anyhow::{Context, Result};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageReader};
use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::probe::Probe;
use std::io::Cursor;
use std::path::Path;
use tracing::{debug, warn};

/// Maximum dimension for cover art (width or height)
/// 300px for maximum Echo Mini compatibility (per user reports)
const MAX_COVER_SIZE: u32 = 300;

/// JPEG quality (0-100) - 75 for smaller file sizes
const JPEG_QUALITY: u8 = 75;

/// Maximum file size for cover art in bytes (200KB)
const MAX_COVER_BYTES: usize = 200 * 1024;

/// Process cover art for device compatibility
///
/// - Decodes the image
/// - Resizes to fit within MAX_COVER_SIZE (500x500)
/// - Encodes as baseline JPEG
/// - Reduces quality if file size exceeds MAX_COVER_BYTES
pub fn process_cover_art(data: &[u8]) -> Result<Vec<u8>> {
    // Load image
    let img = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .context("Failed to guess image format")?
        .decode()
        .context("Failed to decode cover art")?;

    // Resize to fit within MAX_COVER_SIZE
    let img = resize_to_fit(img);

    // Encode as baseline JPEG, reducing quality if file is too large
    let mut quality = JPEG_QUALITY;
    loop {
        let mut output = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut output, quality);
        encoder
            .encode_image(&img)
            .context("Failed to encode cover art as JPEG")?;

        if output.len() <= MAX_COVER_BYTES || quality <= 50 {
            debug!(
                "Processed cover art: {}x{} -> {} bytes (quality {})",
                img.width(),
                img.height(),
                output.len(),
                quality
            );
            return Ok(output);
        }

        // Reduce quality and try again
        warn!(
            "Cover art too large ({} bytes), reducing quality from {} to {}",
            output.len(),
            quality,
            quality - 10
        );
        quality -= 10;
    }
}

/// Resize image to fit within MAX_COVER_SIZE while maintaining aspect ratio
fn resize_to_fit(img: DynamicImage) -> DynamicImage {
    let (width, height) = (img.width(), img.height());

    // Don't resize if already small enough
    if width <= MAX_COVER_SIZE && height <= MAX_COVER_SIZE {
        return img;
    }

    // Calculate new dimensions maintaining aspect ratio
    let (new_width, new_height) = if width > height {
        let ratio = MAX_COVER_SIZE as f64 / width as f64;
        (MAX_COVER_SIZE, (height as f64 * ratio) as u32)
    } else {
        let ratio = MAX_COVER_SIZE as f64 / height as f64;
        ((width as f64 * ratio) as u32, MAX_COVER_SIZE)
    };

    debug!(
        "Resizing cover art: {}x{} -> {}x{}",
        width, height, new_width, new_height
    );

    img.resize(new_width, new_height, FilterType::Lanczos3)
}

/// Embed cover art into an audio file
///
/// Supports MP3, FLAC, OGG, M4A and other formats via lofty
pub fn embed_cover_art(audio_path: &Path, cover_data: &[u8]) -> Result<()> {
    // Process cover art first
    let processed_cover = process_cover_art(cover_data)?;

    // Open the audio file
    let mut tagged_file = Probe::open(audio_path)
        .context("Failed to open audio file")?
        .read()
        .context("Failed to read audio file tags")?;

    // Create the picture
    let picture = Picture::new_unchecked(
        PictureType::CoverFront,
        Some(MimeType::Jpeg),
        None,
        processed_cover,
    );

    // Get the primary tag (or create one)
    let tag = match tagged_file.primary_tag_mut() {
        Some(tag) => tag,
        None => {
            // Try to get any tag, or insert a new one based on file type
            if let Some(tag) = tagged_file.first_tag_mut() {
                tag
            } else {
                // Determine appropriate tag type based on file
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

    // Save the file
    tagged_file
        .save_to_path(audio_path, WriteOptions::default())
        .context("Failed to save audio file with embedded cover")?;

    debug!("Embedded cover art in: {}", audio_path.display());
    Ok(())
}

/// Embed cover art into audio data in memory (before writing to disk)
///
/// Returns the modified audio data with embedded cover art.
/// Uses a temporary file because lofty requires seekable I/O with original data.
pub fn embed_cover_art_in_memory(
    audio_data: &[u8],
    cover_data: &[u8],
    file_extension: &str,
) -> Result<Vec<u8>> {
    use std::fs;
    use std::io::Write;

    // Process cover art first
    let processed_cover = process_cover_art(cover_data)?;

    // Create a temp file with the audio data
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!("nutune_embed_{}.{}", std::process::id(), file_extension));

    // Write audio data to temp file
    {
        let mut temp_file = fs::File::create(&temp_path)
            .context("Failed to create temp file for cover embedding")?;
        temp_file.write_all(audio_data)
            .context("Failed to write audio to temp file")?;
    }

    // Open and modify the temp file
    let mut tagged_file = Probe::open(&temp_path)
        .context("Failed to open temp audio file")?
        .read()
        .context("Failed to read temp audio file")?;

    // Create the picture
    let picture = Picture::new_unchecked(
        PictureType::CoverFront,
        Some(MimeType::Jpeg),
        None,
        processed_cover,
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
    let result = fs::read(&temp_path)
        .context("Failed to read modified audio file")?;

    // Clean up temp file
    let _ = fs::remove_file(&temp_path);

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resize_small_image() {
        // Create a small test image (100x100)
        let img = DynamicImage::new_rgb8(100, 100);
        let resized = resize_to_fit(img);
        assert_eq!(resized.width(), 100);
        assert_eq!(resized.height(), 100);
    }

    #[test]
    fn test_resize_large_image() {
        // Create a large test image (1500x1000)
        let img = DynamicImage::new_rgb8(1500, 1000);
        let resized = resize_to_fit(img);
        assert_eq!(resized.width(), MAX_COVER_SIZE);
        assert!(resized.height() <= MAX_COVER_SIZE);
    }
}
