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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    let filter = if cli.verbose {
        "nutune=debug,reqwest=debug"
    } else {
        "nutune=info"
    };

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| filter.into()))
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();

    match cli.command {
        Commands::Auth {
            url,
            username,
            password,
            force,
        } => {
            cli::commands::auth(url, username, password, force).await?;
        }
        Commands::Devices { detailed } => {
            cli::commands::devices(detailed).await?;
        }
        Commands::Browse { artists, playlists } => {
            cli::commands::browse(artists, playlists).await?;
        }
        Commands::Sync {
            device,
            dry_run,
            parallel,
            no_playlists,
            playlists_only,
        } => {
            cli::commands::sync_to_device(device, dry_run, parallel, no_playlists, playlists_only).await?;
        }
        Commands::Status { device } => {
            cli::commands::status(device).await?;
        }
        Commands::Completion { shell } => {
            cli::commands::completion(shell);
        }
    }

    Ok(())
}
