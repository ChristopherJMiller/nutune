//! Subsonic API HTTP client

use anyhow::{Context, Result};
use reqwest::Client;
use tracing::debug;

use super::auth::generate_auth_params;
use super::models::*;

/// HTTP client for Subsonic REST API
#[derive(Clone)]
pub struct SubsonicClient {
    base_url: String,
    username: String,
    password: String,
    http_client: Client,
}

impl SubsonicClient {
    /// Create a new Subsonic client
    pub fn new(base_url: &str, username: &str, password: &str) -> Result<Self> {
        let base_url = base_url.trim_end_matches('/').to_string();

        let http_client = Client::builder()
            .user_agent("nutune/0.1.0")
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            base_url,
            username: username.to_string(),
            password: password.to_string(),
            http_client,
        })
    }

    /// Build URL with authentication parameters
    fn build_url(&self, endpoint: &str) -> String {
        let params = generate_auth_params(&self.username, &self.password);
        let query: String = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        format!("{}/rest/{}?{}", self.base_url, endpoint, query)
    }

    /// Test connection to Subsonic server
    pub async fn ping(&self) -> Result<bool> {
        let url = self.build_url("ping");
        debug!("Pinging Subsonic server: {}", url);

        let response: SubsonicResponse<()> = self
            .http_client
            .get(&url)
            .send()
            .await
            .context("Failed to connect to Subsonic server")?
            .json()
            .await
            .context("Failed to parse ping response")?;

        if response.subsonic_response.status == "ok" {
            Ok(true)
        } else if let Some(error) = response.subsonic_response.error {
            anyhow::bail!("Subsonic error {}: {}", error.code, error.message)
        } else {
            anyhow::bail!("Unknown Subsonic error")
        }
    }

    /// Get all artists in the library
    pub async fn get_artists(&self) -> Result<Vec<Artist>> {
        let url = self.build_url("getArtists");
        debug!("Fetching artists from: {}", url);

        let response: SubsonicResponse<ArtistsData> = self
            .http_client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch artists")?
            .json()
            .await
            .context("Failed to parse artists response")?;

        self.check_response(&response)?;

        let artists: Vec<Artist> = response
            .subsonic_response
            .data
            .map(|d| {
                d.artists
                    .index
                    .into_iter()
                    .flat_map(|idx| idx.artist)
                    .collect()
            })
            .unwrap_or_default();

        debug!("Found {} artists", artists.len());
        Ok(artists)
    }

    /// Get artist details with albums
    pub async fn get_artist(&self, id: &str) -> Result<ArtistWithAlbums> {
        let url = format!("{}&id={}", self.build_url("getArtist"), id);
        debug!("Fetching artist {}: {}", id, url);

        let response: SubsonicResponse<ArtistData> = self
            .http_client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch artist")?
            .json()
            .await
            .context("Failed to parse artist response")?;

        self.check_response(&response)?;

        response
            .subsonic_response
            .data
            .map(|d| d.artist)
            .ok_or_else(|| anyhow::anyhow!("Artist not found"))
    }

    /// Get album details with songs
    pub async fn get_album(&self, id: &str) -> Result<AlbumWithSongs> {
        let url = format!("{}&id={}", self.build_url("getAlbum"), id);
        debug!("Fetching album {}: {}", id, url);

        let response: SubsonicResponse<AlbumData> = self
            .http_client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch album")?
            .json()
            .await
            .context("Failed to parse album response")?;

        self.check_response(&response)?;

        response
            .subsonic_response
            .data
            .map(|d| d.album)
            .ok_or_else(|| anyhow::anyhow!("Album not found"))
    }

    /// Get all playlists
    pub async fn get_playlists(&self) -> Result<Vec<Playlist>> {
        let url = self.build_url("getPlaylists");
        debug!("Fetching playlists from: {}", url);

        let response: SubsonicResponse<PlaylistsData> = self
            .http_client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch playlists")?
            .json()
            .await
            .context("Failed to parse playlists response")?;

        self.check_response(&response)?;

        let playlists = response
            .subsonic_response
            .data
            .map(|d| d.playlists.playlist)
            .unwrap_or_default();

        debug!("Found {} playlists", playlists.len());
        Ok(playlists)
    }

    /// Get playlist details with songs
    pub async fn get_playlist(&self, id: &str) -> Result<PlaylistWithSongs> {
        let url = format!("{}&id={}", self.build_url("getPlaylist"), id);
        debug!("Fetching playlist {}: {}", id, url);

        let response: SubsonicResponse<PlaylistData> = self
            .http_client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch playlist")?
            .json()
            .await
            .context("Failed to parse playlist response")?;

        self.check_response(&response)?;

        response
            .subsonic_response
            .data
            .map(|d| d.playlist)
            .ok_or_else(|| anyhow::anyhow!("Playlist not found"))
    }

    /// Get download URL for a song (returns URL, doesn't download)
    pub fn get_download_url(&self, id: &str) -> String {
        format!("{}&id={}", self.build_url("download"), id)
    }

    /// Download a song as bytes
    pub async fn download(&self, id: &str) -> Result<bytes::Bytes> {
        let url = self.get_download_url(id);
        debug!("Downloading song {}: {}", id, url);

        let response = self
            .http_client
            .get(&url)
            .send()
            .await
            .context("Failed to download song")?;

        // Check if it's an error response (JSON)
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let bytes = response
            .bytes()
            .await
            .context("Failed to read download response")?;

        // If JSON content type, check for error
        if content_type.contains("json") {
            if let Ok(error) = serde_json::from_slice::<SubsonicResponse<()>>(&bytes) {
                if let Some(err) = error.subsonic_response.error {
                    anyhow::bail!("Download failed: {} (code {})", err.message, err.code);
                }
            }
        }

        Ok(bytes)
    }

    /// Get cover art URL
    pub fn get_cover_art_url(&self, id: &str, size: Option<u32>) -> String {
        let mut url = format!("{}&id={}", self.build_url("getCoverArt"), id);
        if let Some(size) = size {
            url = format!("{}&size={}", url, size);
        }
        url
    }

    /// Download cover art as bytes
    pub async fn get_cover_art(&self, id: &str, size: Option<u32>) -> Result<bytes::Bytes> {
        let url = self.get_cover_art_url(id, size);
        debug!("Fetching cover art {}: {}", id, url);

        let response = self
            .http_client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch cover art")?;

        if !response.status().is_success() {
            anyhow::bail!("Cover art not found (status {})", response.status());
        }

        response
            .bytes()
            .await
            .context("Failed to read cover art response")
    }

    /// Check response status and return error if failed
    fn check_response<T>(&self, response: &SubsonicResponse<T>) -> Result<()> {
        if response.subsonic_response.status != "ok" {
            if let Some(error) = &response.subsonic_response.error {
                anyhow::bail!("Subsonic error {}: {}", error.code, error.message);
            }
            anyhow::bail!("Unknown Subsonic error");
        }
        Ok(())
    }
}
