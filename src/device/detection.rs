//! Device detection using lsblk and udisksctl

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;
use std::process::Command;
use tracing::{debug, info};

use super::config::{generate_device_uuid, DeviceConfigStore, DeviceIdentifiers};

/// Detected removable device
#[derive(Debug, Clone)]
pub struct Device {
    /// Device name (e.g., "sdb1")
    pub name: String,
    /// Volume label (e.g., "FIIO")
    pub label: Option<String>,
    /// Mount point path
    pub mount_point: PathBuf,
    /// Total size in bytes
    pub size: u64,
    /// Free space in bytes
    pub free_space: u64,
    /// Filesystem type (e.g., "vfat", "exfat")
    pub fs_type: String,
    /// Unique identifier for this device (stable across reconnects)
    pub uuid: String,
    /// User-defined friendly name (from config)
    pub friendly_name: Option<String>,
}

impl Device {
    /// Get the display name for this device (friendly name, label, or UUID)
    pub fn display_name(&self) -> String {
        self.friendly_name
            .clone()
            .or_else(|| self.label.clone())
            .unwrap_or_else(|| self.uuid[..8.min(self.uuid.len())].to_string())
    }
}

/// Detects mounted removable devices
pub struct DeviceDetector;

impl DeviceDetector {
    /// Scan for mounted removable devices
    pub async fn scan() -> Result<Vec<Device>> {
        // Load device config for friendly names
        let mut config_store = DeviceConfigStore::load().unwrap_or_default();

        // Run lsblk with JSON output
        let output = Command::new("lsblk")
            .args([
                "-J",
                "-o",
                "NAME,LABEL,MOUNTPOINT,SIZE,FSTYPE,HOTPLUG,FSAVAIL,FSSIZE",
                "-b", // bytes
            ])
            .output()
            .context("Failed to run lsblk")?;

        if !output.status.success() {
            anyhow::bail!(
                "lsblk failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let json_output = String::from_utf8_lossy(&output.stdout);
        debug!("lsblk output: {}", json_output);

        let lsblk: LsblkOutput = serde_json::from_str(&json_output)
            .context("Failed to parse lsblk output")?;

        let mut devices = Vec::new();

        for block_device in lsblk.blockdevices {
            Self::collect_devices(&block_device, &mut devices, &mut config_store);
        }

        // Save config to update last_seen timestamps
        let _ = config_store.save();

        debug!("Found {} removable devices", devices.len());
        Ok(devices)
    }

    /// Recursively collect removable devices from lsblk output
    fn collect_devices(
        block: &BlockDevice,
        devices: &mut Vec<Device>,
        config_store: &mut DeviceConfigStore,
    ) {
        // Check if this is a mounted removable device
        if (block.hotplug == Some(true) || block.hotplug == Some(false))
            && let Some(mountpoint) = &block.mountpoint
                && !mountpoint.is_empty()
                    && mountpoint != "[SWAP]"
                    && !mountpoint.starts_with("/boot")
                {
                    // Try to determine if it's removable
                    // Check if it's a partition of a hotplug device or has typical removable paths
                    let is_removable = block.hotplug == Some(true)
                        || mountpoint.starts_with("/run/media")
                        || mountpoint.starts_with("/media")
                        || mountpoint.starts_with("/mnt");

                    if is_removable {
                        let size = block.size.or(block.fssize).unwrap_or(0);
                        let free_space = block.fsavail.unwrap_or(0);
                        let fs_type = block.fstype.clone().unwrap_or_default();

                        // Generate UUID and get config
                        let uuid =
                            generate_device_uuid(block.label.as_deref(), size, &fs_type);

                        // Get or create device config, update last_seen
                        let identifiers = DeviceIdentifiers {
                            label: block.label.clone(),
                            size_bytes: size,
                            fs_type: fs_type.clone(),
                        };
                        let device_config = config_store.get_or_create(&uuid, identifiers);
                        device_config.last_seen = chrono::Utc::now();
                        let friendly_name = device_config.friendly_name.clone();

                        devices.push(Device {
                            name: block.name.clone(),
                            label: block.label.clone(),
                            mount_point: PathBuf::from(mountpoint),
                            size,
                            free_space,
                            fs_type,
                            uuid,
                            friendly_name,
                        });
                    }
                }

        // Check children
        if let Some(children) = &block.children {
            for child in children {
                Self::collect_devices(child, devices, config_store);
            }
        }
    }

    /// Find a device by name, label, or mount point
    pub async fn find(identifier: &str) -> Result<Option<Device>> {
        let devices = Self::scan().await?;

        // Try exact match on name first
        if let Some(device) = devices.iter().find(|d| d.name == identifier) {
            return Ok(Some(device.clone()));
        }

        // Try label match (case-insensitive)
        if let Some(device) = devices.iter().find(|d| {
            d.label
                .as_ref()
                .is_some_and(|l| l.eq_ignore_ascii_case(identifier))
        }) {
            return Ok(Some(device.clone()));
        }

        // Try mount point match
        if let Some(device) = devices.iter().find(|d| {
            d.mount_point.to_string_lossy() == identifier
                || d.mount_point
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(identifier))
        }) {
            return Ok(Some(device.clone()));
        }

        Ok(None)
    }

    /// Get unmounted but available devices (for offering to mount)
    pub async fn scan_unmounted() -> Result<Vec<UnmountedDevice>> {
        let output = Command::new("lsblk")
            .args(["-J", "-o", "NAME,LABEL,SIZE,FSTYPE,HOTPLUG", "-b"])
            .output()
            .context("Failed to run lsblk")?;

        if !output.status.success() {
            anyhow::bail!("lsblk failed: {}", String::from_utf8_lossy(&output.stderr));
        }

        let json_output = String::from_utf8_lossy(&output.stdout);
        let lsblk: LsblkOutput = serde_json::from_str(&json_output)
            .context("Failed to parse lsblk output")?;

        let mut devices = Vec::new();
        for block_device in lsblk.blockdevices {
            Self::collect_unmounted(&block_device, &mut devices);
        }

        Ok(devices)
    }

    fn collect_unmounted(block: &BlockDevice, devices: &mut Vec<UnmountedDevice>) {
        // Check if this is an unmounted removable device with a filesystem
        if block.hotplug == Some(true) && block.fstype.is_some() && block.mountpoint.is_none() {
            devices.push(UnmountedDevice {
                name: block.name.clone(),
                label: block.label.clone(),
                size: block.size.unwrap_or(0),
                fs_type: block.fstype.clone().unwrap_or_default(),
            });
        }

        if let Some(children) = &block.children {
            for child in children {
                Self::collect_unmounted(child, devices);
            }
        }
    }

    /// Mount a device using udisksctl (triggers polkit GUI prompt on KDE/GNOME)
    pub async fn mount(device_name: &str) -> Result<PathBuf> {
        info!("Mounting {} via udisksctl (may show auth dialog)...", device_name);

        let output = Command::new("udisksctl")
            .args(["mount", "-b", &format!("/dev/{}", device_name)])
            .output()
            .context("Failed to run udisksctl")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to mount device: {}", stderr);
        }

        // Parse mount point from output like "Mounted /dev/sda1 at /run/media/user/LABEL"
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mount_point = stdout
            .split(" at ")
            .nth(1)
            .map(|s| s.trim().trim_end_matches('.'))
            .ok_or_else(|| anyhow::anyhow!("Could not parse mount point from: {}", stdout))?;

        info!("Mounted at: {}", mount_point);
        Ok(PathBuf::from(mount_point))
    }
}

/// Unmounted device that can be mounted
#[derive(Debug, Clone)]
pub struct UnmountedDevice {
    pub name: String,
    pub label: Option<String>,
    pub size: u64,
    pub fs_type: String,
}

// JSON structures for lsblk output

#[derive(Debug, Deserialize)]
struct LsblkOutput {
    blockdevices: Vec<BlockDevice>,
}

#[derive(Debug, Deserialize)]
struct BlockDevice {
    name: String,
    label: Option<String>,
    mountpoint: Option<String>,
    size: Option<u64>,
    fstype: Option<String>,
    hotplug: Option<bool>,
    fsavail: Option<u64>,
    fssize: Option<u64>,
    children: Option<Vec<BlockDevice>>,
}
