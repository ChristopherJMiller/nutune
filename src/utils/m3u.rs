//! M3U playlist generation

/// Generate an M3U playlist file content
///
/// Uses relative paths (just filenames) for maximum compatibility
/// with portable devices like FiiO players.
pub fn generate_m3u(tracks: &[String]) -> String {
    let mut content = String::from("#EXTM3U\n");
    for track in tracks {
        content.push_str(track);
        content.push('\n');
    }
    content
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_m3u_empty() {
        let result = generate_m3u(&[]);
        assert_eq!(result, "#EXTM3U\n");
    }

    #[test]
    fn test_generate_m3u_tracks() {
        let tracks = vec![
            "01 - Track One.flac".to_string(),
            "02 - Track Two.flac".to_string(),
        ];
        let result = generate_m3u(&tracks);
        assert_eq!(result, "#EXTM3U\n01 - Track One.flac\n02 - Track Two.flac\n");
    }
}
