//! Device detection and storage module

pub mod config;
pub mod detection;
pub mod manifest;
pub mod storage;

pub use detection::{Device, DeviceDetector, UnmountedDevice};
pub use manifest::{SyncManifest, SyncedAlbum, SyncedPlaylist};
pub use storage::DeviceStorage;
