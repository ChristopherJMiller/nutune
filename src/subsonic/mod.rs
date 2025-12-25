//! Subsonic API client module

pub mod auth;
pub mod client;
pub mod models;

pub use auth::generate_auth_params;
pub use client::SubsonicClient;
pub use models::*;
