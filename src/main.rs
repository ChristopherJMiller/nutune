//! nutune - Sync music from Subsonic to portable devices

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod cli;
mod device;
mod subsonic;
mod sync;
mod browse;
mod utils;

use cli::{Cli, Commands};
use utils::ConditionalStderrLayer;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging with TUI-aware conditional layer
    // When TUI mode is active, stderr output is suppressed to prevent display corruption
    let filter = if cli.verbose {
        "nutune=debug,reqwest=debug"
    } else {
        "nutune=info"
    };

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| filter.into()))
        .with(ConditionalStderrLayer::new(
            tracing_subscriber::fmt::layer().with_target(false)
        ))
        .init();

    match cli.command {
        // Default: launch TUI browser when no command is specified
        None => {
            cli::commands::browse(false, false).await?;
        }
        Some(Commands::Auth {
            url,
            username,
            password,
            force,
        }) => {
            cli::commands::auth(url, username, password, force).await?;
        }
        Some(Commands::Devices { detailed }) => {
            cli::commands::devices(detailed).await?;
        }
        Some(Commands::Browse { artists, playlists }) => {
            cli::commands::browse(artists, playlists).await?;
        }
        Some(Commands::Sync {
            device,
            dry_run,
            parallel,
            no_playlists,
            playlists_only,
        }) => {
            cli::commands::sync_to_device(device, dry_run, parallel, no_playlists, playlists_only).await?;
        }
        Some(Commands::Status { device }) => {
            cli::commands::status(device).await?;
        }
        Some(Commands::Completion { shell }) => {
            cli::commands::completion(shell);
        }
    }

    Ok(())
}
