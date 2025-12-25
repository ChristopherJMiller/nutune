//! Subsonic API authentication
//!
//! Generates authentication tokens using the MD5(password + salt) scheme
//! as specified by Subsonic API version 1.13.0+

use md5::{Digest, Md5};
use rand::Rng;

/// API version to use for requests
pub const API_VERSION: &str = "1.16.1";

/// Client identifier
pub const CLIENT_NAME: &str = "nutune";

/// Generate authentication parameters for Subsonic API requests
///
/// Returns a vector of (key, value) pairs to include in the request URL:
/// - u: username
/// - t: token (MD5 hash of password + salt)
/// - s: random salt
/// - v: API version
/// - c: client identifier
/// - f: response format (json)
pub fn generate_auth_params(username: &str, password: &str) -> Vec<(String, String)> {
    let salt = generate_salt();
    let token = generate_token(password, &salt);

    vec![
        ("u".to_string(), username.to_string()),
        ("t".to_string(), token),
        ("s".to_string(), salt),
        ("v".to_string(), API_VERSION.to_string()),
        ("c".to_string(), CLIENT_NAME.to_string()),
        ("f".to_string(), "json".to_string()),
    ]
}

/// Generate a random salt string (16 alphanumeric characters)
fn generate_salt() -> String {
    rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(16)
        .map(char::from)
        .collect()
}

/// Generate token: MD5(password + salt) as hex string
fn generate_token(password: &str, salt: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(password.as_bytes());
    hasher.update(salt.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_salt_length() {
        let salt = generate_salt();
        assert_eq!(salt.len(), 16);
    }

    #[test]
    fn test_generate_token_format() {
        let token = generate_token("password", "salt123");
        // MD5 produces 32 hex characters
        assert_eq!(token.len(), 32);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_auth_params_contains_all_fields() {
        let params = generate_auth_params("user", "pass");
        let keys: Vec<_> = params.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"u"));
        assert!(keys.contains(&"t"));
        assert!(keys.contains(&"s"));
        assert!(keys.contains(&"v"));
        assert!(keys.contains(&"c"));
        assert!(keys.contains(&"f"));
    }
}
