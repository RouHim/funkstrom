#[cfg(test)]
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct TrackMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub file_path: String,
}

impl TrackMetadata {
    /// Create metadata from filename when tags are unavailable
    #[cfg(test)]
    fn from_filename(path: &Path) -> Self {
        let title = Self::default_title(path);
        let file_path = path.to_string_lossy().to_string();

        Self {
            title,
            artist: "Unknown Artist".to_string(),
            album: "Unknown Album".to_string(),
            file_path,
        }
    }

    /// Get default title from filename
    #[cfg(test)]
    fn default_title(path: &Path) -> String {
        path.file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Unknown Track".to_string())
    }

    /// Format metadata for ICY (Icecast) protocol
    /// Format: "Artist - Title"
    pub fn to_icy_metadata(&self) -> String {
        format!("{} - {}", self.artist, self.title)
    }

    /// Format metadata as JSON
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "title": self.title,
            "artist": self.artist,
            "album": self.album,
            "file_path": self.file_path,
        })
        .to_string()
    }
}

impl Default for TrackMetadata {
    fn default() -> Self {
        Self {
            title: "Unknown Track".to_string(),
            artist: "Unknown Artist".to_string(),
            album: "Unknown Album".to_string(),
            file_path: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_from_filename() {
        let path = PathBuf::from("/music/test_song.mp3");
        let metadata = TrackMetadata::from_filename(&path);

        assert_eq!(metadata.title, "test_song");
        assert_eq!(metadata.artist, "Unknown Artist");
        assert_eq!(metadata.album, "Unknown Album");
    }

    #[test]
    fn test_to_icy_metadata() {
        let metadata = TrackMetadata {
            title: "Test Song".to_string(),
            artist: "Test Artist".to_string(),
            album: "Test Album".to_string(),
            file_path: "/music/test.mp3".to_string(),
        };

        assert_eq!(metadata.to_icy_metadata(), "Test Artist - Test Song");
    }

    #[test]
    fn test_default_title() {
        let path = PathBuf::from("/music/my song.flac");
        let title = TrackMetadata::default_title(&path);
        assert_eq!(title, "my song");
    }
}
