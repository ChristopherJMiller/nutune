//! CLI command handlers

use anyhow::Result;
use clap_complete::generate;
use colored::Colorize;
use std::io;

use super::AuthManager;
use crate::browse;
use crate::device::{DeviceDetector, SyncManifest};
use crate::subsonic::SubsonicClient;
use crate::sync::SyncEngine;

/// Handle the `auth` command
pub async fn auth(
    url: Option<String>,
    username: Option<String>,
    password: Option<String>,
    force: bool,
) -> Result<()> {
    println!("{}", "Configuring Subsonic credentials...".cyan());

    let creds = AuthManager::authenticate(url, username, password, force).await?;

    println!();
    println!("{}", "Authentication successful!".green().bold());
    println!("  Server: {}", creds.url);
    println!("  User: {}", creds.username);
    println!();
    println!("Credentials stored securely in system keyring.");

    Ok(())
}

/// Handle the `devices` command
pub async fn devices(detailed: bool) -> Result<()> {
    println!("{}", "Scanning for devices...".cyan());
    println!();

    let mounted_devices = DeviceDetector::scan().await?;
    let unmounted_devices = DeviceDetector::scan_unmounted().await.unwrap_or_default();

    if mounted_devices.is_empty() && unmounted_devices.is_empty() {
        println!("{}", "No removable devices found.".yellow());
        println!("Make sure your device is connected.");
        return Ok(());
    }

    // Show mounted devices
    if !mounted_devices.is_empty() {
        println!("{}", "Mounted devices:".green().bold());
        for device in &mounted_devices {
            let label = device.label.as_deref().unwrap_or("(no label)");
            let name = &device.name;

            if detailed {
                let free_gb = device.free_space as f64 / 1_073_741_824.0;
                let total_gb = device.size as f64 / 1_073_741_824.0;
                let used_percent = if device.size > 0 {
                    ((device.size - device.free_space) as f64 / device.size as f64) * 100.0
                } else {
                    0.0
                };

                println!("  {} {}", "Name:".bold(), name);
                println!("    Label: {}", label);
                println!("    Mount: {}", device.mount_point.display());
                println!("    Type:  {}", device.fs_type);
                println!(
                    "    Space: {:.1} GB free / {:.1} GB total ({:.0}% used)",
                    free_gb, total_gb, used_percent
                );

                // Check for nutune manifest
                if let Ok(Some(manifest)) = SyncManifest::load(&device.mount_point) {
                    println!(
                        "    Synced: {} albums, {} playlists (last: {})",
                        manifest.synced_albums.len(),
                        manifest.synced_playlists.len(),
                        manifest.last_sync.format("%Y-%m-%d %H:%M")
                    );
                }
                println!();
            } else {
                println!(
                    "  {} - {} ({})",
                    name.green(),
                    label,
                    device.mount_point.display()
                );
            }
        }
    }

    // Show unmounted devices
    if !unmounted_devices.is_empty() {
        println!();
        println!("{}", "Unmounted devices (can be mounted):".yellow().bold());
        for device in &unmounted_devices {
            let label = device.label.as_deref().unwrap_or("(no label)");
            let size_gb = device.size as f64 / 1_073_741_824.0;
            println!(
                "  {} - {} ({:.1} GB, {})",
                device.name.yellow(),
                label,
                size_gb,
                device.fs_type
            );
        }
        println!();
        println!(
            "To mount a device, use: {}",
            "udisksctl mount -b /dev/<name>".cyan()
        );
        println!("Or run {} and select the device to mount.", "nutune sync".cyan());
    }

    if !detailed && !mounted_devices.is_empty() {
        println!();
        println!("Use {} for more details.", "--detailed".cyan());
    }

    Ok(())
}

/// Handle the `browse` command
pub async fn browse(_start_artists: bool, start_playlists: bool) -> Result<()> {
    let creds = AuthManager::load().map_err(|_| {
        anyhow::anyhow!("No credentials found. Run 'nutune auth' first to configure.")
    })?;

    let client = SubsonicClient::new(&creds.url, &creds.username, &creds.password)?;

    // Verify connection
    println!("{}", "Connecting to Subsonic server...".cyan());
    client.ping().await?;
    println!("{}", "Connected!".green());
    println!();

    // Run interactive browser
    let initial_view = if start_playlists {
        browse::BrowseView::Playlists
    } else {
        browse::BrowseView::Artists
    };

    let result = browse::run_browser(&client, initial_view).await?;

    match result {
        browse::BrowseResult::SelectionOnly(selection) => {
            if selection.is_empty() {
                println!("{}", "No items selected.".yellow());
                return Ok(());
            }

            println!();
            println!(
                "Selected {} album(s) and {} playlist(s).",
                selection.album_count(),
                selection.playlist_count()
            );
            println!("Run {} to sync to a device.", "nutune sync <device>".cyan());

            // Save selection for sync command
            selection.save()?;
        }
        browse::BrowseResult::SyncToDevice { selection, device } => {
            // Sync was already completed in the TUI - just print summary
            println!();
            println!(
                "{} to {}",
                "Sync completed".green().bold(),
                device.label.as_deref().unwrap_or(&device.name).cyan()
            );
            println!(
                "  Selected: {} album(s), {} playlist(s)",
                selection.album_count(),
                selection.playlist_count()
            );
        }
    }

    Ok(())
}

/// Handle the `sync` command
pub async fn sync_to_device(
    device_id: String,
    dry_run: bool,
    parallel: usize,
    no_playlists: bool,
    playlists_only: bool,
) -> Result<()> {
    // Load credentials
    let creds = AuthManager::load().map_err(|_| {
        anyhow::anyhow!("No credentials found. Run 'nutune auth' first to configure.")
    })?;

    // Find device - check mounted first, then unmounted
    let device = match DeviceDetector::find(&device_id).await? {
        Some(d) => d,
        None => {
            // Check if it's an unmounted device we can mount
            let unmounted = DeviceDetector::scan_unmounted().await?;
            let unmounted_match = unmounted.iter().find(|d| {
                d.name == device_id
                    || d.label
                        .as_ref()
                        .is_some_and(|l| l.eq_ignore_ascii_case(&device_id))
            });

            if let Some(um) = unmounted_match {
                println!(
                    "Device '{}' is not mounted. Mounting via udisksctl...",
                    um.label.as_deref().unwrap_or(&um.name)
                );
                println!("{}", "(A system authentication dialog may appear)".yellow());

                let _mount_point = DeviceDetector::mount(&um.name).await?;

                // Re-scan to get full device info
                DeviceDetector::find(&um.name)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("Device mounted but not found"))?
            } else {
                anyhow::bail!(
                    "Device '{}' not found. Run 'nutune devices' to list available devices.",
                    device_id
                );
            }
        }
    };

    println!("Syncing to: {} ({})", device.name.green(), device.mount_point.display());

    // Load selection
    let selection = crate::subsonic::SyncSelection::load()?;
    if selection.is_empty() {
        println!("{}", "No items selected. Run 'nutune browse' first to select music.".yellow());
        return Ok(());
    }

    // Filter selection based on flags
    let selection = if no_playlists {
        crate::subsonic::SyncSelection {
            albums: selection.albums,
            playlists: vec![],
        }
    } else if playlists_only {
        crate::subsonic::SyncSelection {
            albums: vec![],
            playlists: selection.playlists,
        }
    } else {
        selection
    };

    println!(
        "Syncing {} album(s) and {} playlist(s)...",
        selection.album_count(),
        selection.playlist_count()
    );

    if dry_run {
        println!();
        println!("{}", "[DRY RUN] Would sync:".yellow());
        for album in &selection.albums {
            let artist = album.artist.as_deref().unwrap_or("Unknown Artist");
            println!("  Album: {} - {}", artist, album.name);
        }
        for playlist in &selection.playlists {
            println!("  Playlist: {}", playlist.name);
        }
        return Ok(());
    }

    // Create client and sync engine
    let client = SubsonicClient::new(&creds.url, &creds.username, &creds.password)?;
    let mut engine = SyncEngine::new(client, device.mount_point.clone(), parallel)?;

    // Run sync
    let result = engine.sync(&selection).await?;

    println!();
    println!("{}", "Sync complete!".green().bold());
    println!(
        "  Albums synced: {}",
        result.albums_synced
    );
    println!(
        "  Playlists synced: {}",
        result.playlists_synced
    );
    println!(
        "  Tracks downloaded: {}",
        result.tracks_downloaded
    );
    println!(
        "  Total size: {:.1} MB",
        result.bytes_downloaded as f64 / 1_048_576.0
    );

    Ok(())
}

/// Handle the `status` command
pub async fn status(device_id: Option<String>) -> Result<()> {
    let devices = if let Some(id) = device_id {
        let device = DeviceDetector::find(&id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Device '{}' not found", id))?;
        vec![device]
    } else {
        DeviceDetector::scan().await?
    };

    if devices.is_empty() {
        println!("{}", "No devices found.".yellow());
        return Ok(());
    }

    for device in devices {
        let label = device.label.as_deref().unwrap_or("(no label)");
        println!("{} - {}", device.name.green().bold(), label);
        println!("  Mount: {}", device.mount_point.display());

        match SyncManifest::load(&device.mount_point)? {
            Some(manifest) => {
                println!("  Last sync: {}", manifest.last_sync.format("%Y-%m-%d %H:%M:%S"));
                println!("  Synced albums: {}", manifest.synced_albums.len());
                for album in &manifest.synced_albums {
                    println!("    - {} - {}", album.artist, album.album);
                }
                println!("  Synced playlists: {}", manifest.synced_playlists.len());
                for playlist in &manifest.synced_playlists {
                    println!("    - {} ({} tracks)", playlist.name, playlist.track_count);
                }
            }
            None => {
                println!("  {}", "No nutune sync history found.".yellow());
            }
        }
        println!();
    }

    Ok(())
}

/// Handle the `completion` command
pub fn completion(shell: clap_complete::Shell) {
    let mut cmd = super::Cli::command();
    generate(shell, &mut cmd, "nutune", &mut io::stdout());
}

// Extension trait for Cli to get clap Command
impl super::Cli {
    fn command() -> clap::Command {
        <Self as clap::CommandFactory>::command()
    }
}
