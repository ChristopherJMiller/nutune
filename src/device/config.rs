//! Device configuration persistence
//!
//! Stores device preferences and friendly names in ~/.config/nutune/devices.json
//! Devices are identified by a UUID generated from their properties (label, size, fs_type).

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tracing::debug;

/// Persistent device configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    /// User-defined friendly name for the device
    pub friendly_name: Option<String>,
    /// First time this device was seen
    pub first_seen: DateTime<Utc>,
    /// Last time this device was seen
    pub last_seen: DateTime<Utc>,
    /// Device identifiers used for matching
    pub identifiers: DeviceIdentifiers,
}

/// Identifying properties of a device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceIdentifiers {
    /// Volume label (if any)
    pub label: Option<String>,
    /// Total size in bytes
    pub size_bytes: u64,
    /// Filesystem type
    pub fs_type: String,
}

/// Device configuration store
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeviceConfigStore {
    /// Config format version
    pub version: u32,
    /// Devices indexed by UUID
    pub devices: HashMap<String, DeviceConfig>,
}

impl DeviceConfigStore {
    /// Load the device config store from disk
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;

        if !path.exists() {
            debug!("No device config found, using defaults");
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read device config from {:?}", path))?;

        let store: Self = serde_json::from_str(&contents)
            .with_context(|| "Failed to parse device config")?;

        debug!("Loaded {} devices from config", store.devices.len());
        Ok(store)
    }

    /// Save the device config store to disk
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create config directory {:?}", parent))?;
        }

        let contents = serde_json::to_string_pretty(self)
            .context("Failed to serialize device config")?;

        fs::write(&path, contents)
            .with_context(|| format!("Failed to write device config to {:?}", path))?;

        debug!("Saved {} devices to config", self.devices.len());
        Ok(())
    }

    /// Get the config file path
    fn config_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?;
        Ok(config_dir.join("nutune").join("devices.json"))
    }

    /// Get or create config for a device
    pub fn get_or_create(&mut self, uuid: &str, identifiers: DeviceIdentifiers) -> &mut DeviceConfig {
        let now = Utc::now();
        self.devices.entry(uuid.to_string()).or_insert_with(|| {
            debug!("Creating new device config for UUID: {}", uuid);
            DeviceConfig {
                friendly_name: None,
                first_seen: now,
                last_seen: now,
                identifiers,
            }
        })
    }
}

impl Default for DeviceConfig {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            friendly_name: None,
            first_seen: now,
            last_seen: now,
            identifiers: DeviceIdentifiers {
                label: None,
                size_bytes: 0,
                fs_type: String::new(),
            },
        }
    }
}

/// Generate a stable UUID for a device based on its properties
///
/// Uses SHA256 hash of label + size + fs_type, taking first 12 hex chars.
/// This allows devices to be recognized even if mount points change.
pub fn generate_device_uuid(label: Option<&str>, size: u64, fs_type: &str) -> String {
    let mut hasher = Sha256::new();

    // Include label (or empty string)
    hasher.update(label.unwrap_or(""));
    hasher.update(b"|");

    // Include size
    hasher.update(size.to_string().as_bytes());
    hasher.update(b"|");

    // Include filesystem type
    hasher.update(fs_type.as_bytes());

    let result = hasher.finalize();
    // Take first 12 hex chars (6 bytes)
    hex::encode(&result[..6])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_device_uuid() {
        let uuid1 = generate_device_uuid(Some("FIIO"), 64424509440, "exfat");
        let uuid2 = generate_device_uuid(Some("FIIO"), 64424509440, "exfat");
        let uuid3 = generate_device_uuid(Some("OTHER"), 64424509440, "exfat");

        // Same properties should generate same UUID
        assert_eq!(uuid1, uuid2);
        // Different properties should generate different UUID
        assert_ne!(uuid1, uuid3);
        // UUID should be 12 hex characters
        assert_eq!(uuid1.len(), 12);
    }

    #[test]
    fn test_generate_uuid_no_label() {
        let uuid = generate_device_uuid(None, 32000000000, "vfat");
        assert_eq!(uuid.len(), 12);
    }
}
