//! Keyring-based credential storage for Subsonic

use anyhow::{Context, Result};
use dialoguer::{Input, Password};
use keyring::Entry;
use tracing::{debug, info};

const KEYRING_SERVICE: &str = "nutune";

/// Subsonic server credentials
#[derive(Debug, Clone)]
pub struct SubsonicCredentials {
    pub url: String,
    pub username: String,
    pub password: String,
}

/// Manages authentication credentials storage
pub struct AuthManager;

impl AuthManager {
    /// Authenticate with Subsonic server
    ///
    /// Tries to load credentials from keyring first, or prompts for new ones.
    /// Verifies credentials work before storing.
    pub async fn authenticate(
        url: Option<String>,
        username: Option<String>,
        password: Option<String>,
        force: bool,
    ) -> Result<SubsonicCredentials> {
        // Try to load existing credentials if not forcing re-auth
        if !force {
            if let Ok(creds) = Self::load() {
                info!("Found existing credentials in keyring");
                return Ok(creds);
            }
        } else {
            debug!("Force flag set, ignoring stored credentials");
        }

        // Prompt for missing values
        let url = url.unwrap_or_else(|| {
            Input::new()
                .with_prompt("Subsonic server URL")
                .interact_text()
                .expect("Failed to read URL")
        });

        let username = username.unwrap_or_else(|| {
            Input::new()
                .with_prompt("Username")
                .interact_text()
                .expect("Failed to read username")
        });

        let password = password.unwrap_or_else(|| {
            Password::new()
                .with_prompt("Password")
                .interact()
                .expect("Failed to read password")
        });

        let creds = SubsonicCredentials {
            url: url.trim_end_matches('/').to_string(),
            username,
            password,
        };

        // Verify credentials work
        Self::verify(&creds).await?;

        // Store credentials
        Self::store(&creds)?;
        info!("Credentials stored in keyring");

        Ok(creds)
    }

    /// Load credentials from keyring
    pub fn load() -> Result<SubsonicCredentials> {
        let url = Self::get_entry("url")?
            .get_password()
            .context("No Subsonic URL in keyring")?;

        let username = Self::get_entry("username")?
            .get_password()
            .context("No Subsonic username in keyring")?;

        let password = Self::get_entry("password")?
            .get_password()
            .context("No Subsonic password in keyring")?;

        Ok(SubsonicCredentials {
            url,
            username,
            password,
        })
    }

    /// Store credentials in keyring
    pub fn store(creds: &SubsonicCredentials) -> Result<()> {
        Self::get_entry("url")?
            .set_password(&creds.url)
            .context("Failed to store URL in keyring")?;

        Self::get_entry("username")?
            .set_password(&creds.username)
            .context("Failed to store username in keyring")?;

        Self::get_entry("password")?
            .set_password(&creds.password)
            .context("Failed to store password in keyring")?;

        debug!("Credentials stored in keyring");
        Ok(())
    }

    /// Clear stored credentials
    pub fn clear() -> Result<()> {
        let _ = Self::get_entry("url")?.delete_credential();
        let _ = Self::get_entry("username")?.delete_credential();
        let _ = Self::get_entry("password")?.delete_credential();
        info!("Credentials cleared from keyring");
        Ok(())
    }

    /// Check if credentials exist in keyring
    pub fn exists() -> bool {
        Self::get_entry("url")
            .and_then(|e| e.get_password().map_err(|e| anyhow::anyhow!("{}", e)))
            .is_ok()
    }

    /// Verify credentials by pinging the Subsonic server
    async fn verify(creds: &SubsonicCredentials) -> Result<()> {
        use crate::subsonic::SubsonicClient;

        debug!("Verifying credentials against {}", creds.url);

        let client = SubsonicClient::new(&creds.url, &creds.username, &creds.password)?;
        client.ping().await.context("Failed to verify credentials")?;

        info!("Credentials verified successfully");
        Ok(())
    }

    /// Get a keyring entry for a given key
    fn get_entry(key: &str) -> Result<Entry> {
        let entry_key = format!("subsonic:{}", key);
        Entry::new(KEYRING_SERVICE, &entry_key).context("Failed to access keyring")
    }
}
