//! CLI module for nutune

use clap::{Parser, Subcommand};

pub mod auth;
pub mod commands;

pub use auth::AuthManager;

#[derive(Parser, Debug)]
#[command(name = "nutune", about = "Sync Subsonic music to portable devices")]
#[command(version, author)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    pub verbose: bool,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Configure Subsonic server credentials
    Auth {
        /// Subsonic server URL
        #[arg(long, env = "SUBSONIC_URL")]
        url: Option<String>,

        /// Username
        #[arg(short, long, env = "SUBSONIC_USER")]
        username: Option<String>,

        /// Password
        #[arg(short, long, env = "SUBSONIC_PASS")]
        password: Option<String>,

        /// Force re-authentication (ignore stored credentials)
        #[arg(long)]
        force: bool,
    },

    /// List detected portable devices
    Devices {
        /// Show detailed information (free space, filesystem type)
        #[arg(short, long)]
        detailed: bool,
    },

    /// Interactive browse and select music to sync
    Browse {
        /// Start with artists view
        #[arg(long, conflicts_with = "playlists")]
        artists: bool,

        /// Start with playlists view
        #[arg(long, conflicts_with = "artists")]
        playlists: bool,
    },

    /// Sync selected content to device
    Sync {
        /// Device identifier (name, label, or mount point from `devices` command)
        #[arg(value_name = "DEVICE")]
        device: String,

        /// Dry run - show what would be synced without downloading
        #[arg(long)]
        dry_run: bool,

        /// Number of parallel downloads
        #[arg(short, long, default_value = "4")]
        parallel: usize,

        /// Skip playlists, only sync artist/album folders
        #[arg(long)]
        no_playlists: bool,

        /// Skip artist folders, only sync playlists
        #[arg(long)]
        playlists_only: bool,
    },

    /// Show sync status for a device
    Status {
        /// Device identifier (optional, shows all if omitted)
        device: Option<String>,
    },

    /// Generate shell completions
    Completion {
        /// Shell to generate completions for
        shell: clap_complete::Shell,
    },
}
