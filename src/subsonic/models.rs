//! Subsonic API response models

use serde::{Deserialize, Serialize};

/// Wrapper for all Subsonic API responses
#[derive(Debug, Clone, Deserialize)]
pub struct SubsonicResponse<T> {
    #[serde(rename = "subsonic-response")]
    pub subsonic_response: SubsonicResponseInner<T>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SubsonicResponseInner<T> {
    pub status: String,
    pub version: String,
    #[serde(flatten)]
    pub data: Option<T>,
    pub error: Option<SubsonicError>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SubsonicError {
    pub code: i32,
    pub message: String,
}

// Artist index response (getArtists)
#[derive(Debug, Clone, Deserialize)]
pub struct ArtistsData {
    pub artists: ArtistsIndex,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ArtistsIndex {
    #[serde(default)]
    pub index: Vec<ArtistIndex>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ArtistIndex {
    pub name: String,
    #[serde(default)]
    pub artist: Vec<Artist>,
}

/// Artist from the library
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artist {
    pub id: String,
    pub name: String,
    #[serde(rename = "albumCount")]
    pub album_count: Option<u32>,
    #[serde(rename = "coverArt")]
    pub cover_art: Option<String>,
}

// Artist with albums response (getArtist)
#[derive(Debug, Clone, Deserialize)]
pub struct ArtistData {
    pub artist: ArtistWithAlbums,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ArtistWithAlbums {
    pub id: String,
    pub name: String,
    #[serde(rename = "albumCount")]
    pub album_count: Option<u32>,
    #[serde(rename = "coverArt")]
    pub cover_art: Option<String>,
    #[serde(default)]
    pub album: Vec<Album>,
}

/// Album from the library
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Album {
    pub id: String,
    pub name: String,
    pub artist: Option<String>,
    #[serde(rename = "artistId")]
    pub artist_id: Option<String>,
    #[serde(rename = "coverArt")]
    pub cover_art: Option<String>,
    #[serde(rename = "songCount")]
    pub song_count: Option<u32>,
    pub duration: Option<u32>,
    pub year: Option<u32>,
    pub genre: Option<String>,
}

// Album with songs response (getAlbum)
#[derive(Debug, Clone, Deserialize)]
pub struct AlbumData {
    pub album: AlbumWithSongs,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AlbumWithSongs {
    pub id: String,
    pub name: String,
    pub artist: Option<String>,
    #[serde(rename = "artistId")]
    pub artist_id: Option<String>,
    #[serde(rename = "coverArt")]
    pub cover_art: Option<String>,
    #[serde(rename = "songCount")]
    pub song_count: Option<u32>,
    pub duration: Option<u32>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    #[serde(default)]
    pub song: Vec<Song>,
}

/// Song/track from the library
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Song {
    pub id: String,
    pub title: String,
    pub album: Option<String>,
    #[serde(rename = "albumId")]
    pub album_id: Option<String>,
    pub artist: Option<String>,
    #[serde(rename = "artistId")]
    pub artist_id: Option<String>,
    pub track: Option<u32>,
    #[serde(rename = "discNumber")]
    pub disc_number: Option<u32>,
    pub duration: Option<u32>,
    pub size: Option<u64>,
    pub suffix: Option<String>,
    #[serde(rename = "contentType")]
    pub content_type: Option<String>,
    #[serde(rename = "coverArt")]
    pub cover_art: Option<String>,
    pub path: Option<String>,
}

// Playlists response (getPlaylists)
#[derive(Debug, Clone, Deserialize)]
pub struct PlaylistsData {
    pub playlists: PlaylistsList,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlaylistsList {
    #[serde(default)]
    pub playlist: Vec<Playlist>,
}

/// Playlist metadata
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    #[serde(rename = "songCount")]
    pub song_count: Option<u32>,
    pub duration: Option<u32>,
    pub owner: Option<String>,
    pub public: Option<bool>,
    #[serde(rename = "coverArt")]
    pub cover_art: Option<String>,
}

// Playlist with songs response (getPlaylist)
#[derive(Debug, Clone, Deserialize)]
pub struct PlaylistData {
    pub playlist: PlaylistWithSongs,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlaylistWithSongs {
    pub id: String,
    pub name: String,
    #[serde(rename = "songCount")]
    pub song_count: Option<u32>,
    pub duration: Option<u32>,
    pub owner: Option<String>,
    pub public: Option<bool>,
    #[serde(rename = "coverArt")]
    pub cover_art: Option<String>,
    #[serde(default, rename = "entry")]
    pub songs: Vec<Song>,
}

/// Selection of content to sync
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncSelection {
    pub albums: Vec<Album>,
    pub playlists: Vec<Playlist>,
}

impl SyncSelection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.albums.is_empty() && self.playlists.is_empty()
    }

    pub fn album_count(&self) -> usize {
        self.albums.len()
    }

    pub fn playlist_count(&self) -> usize {
        self.playlists.len()
    }
}
