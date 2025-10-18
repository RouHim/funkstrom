use log::{debug, warn};
use std::fs;
use std::path::{Path, PathBuf};

pub struct M3uParser;

impl M3uParser {
    pub fn parse(playlist_path: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
        if !playlist_path.exists() {
            return Err(format!("M3U playlist not found: {:?}", playlist_path).into());
        }

        let content = fs::read_to_string(playlist_path)?;
        let mut tracks = Vec::new();
        let playlist_dir = playlist_path
            .parent()
            .ok_or("Failed to get playlist directory")?;

        for line in content.lines() {
            let line = line.trim();

            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let track_path = if Path::new(line).is_absolute() {
                PathBuf::from(line)
            } else {
                playlist_dir.join(line)
            };

            if track_path.exists() {
                debug!("Found track in M3U: {:?}", track_path);
                tracks.push(track_path);
            } else {
                warn!("Track file not found: {:?}", track_path);
            }
        }

        if tracks.is_empty() {
            return Err(
                format!("No valid tracks found in M3U playlist: {:?}", playlist_path).into(),
            );
        }

        Ok(tracks)
    }

    pub fn validate_playlist(playlist_path: &Path) -> Result<usize, Box<dyn std::error::Error>> {
        let tracks = Self::parse(playlist_path)?;
        Ok(tracks.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    fn given_test_tracks_in_directory(dir: &Path, count: usize) -> Vec<PathBuf> {
        let mut tracks = Vec::new();
        for i in 0..count {
            let track_path = dir.join(format!("track{}.mp3", i + 1));
            File::create(&track_path).unwrap();
            tracks.push(track_path);
        }
        tracks
    }

    #[test]
    fn given_simple_m3u_playlist_when_parsed_then_returns_all_tracks() {
        let temp_dir = TempDir::new().unwrap();
        let tracks = given_test_tracks_in_directory(temp_dir.path(), 3);

        let playlist_path = temp_dir.path().join("test.m3u");
        let mut file = File::create(&playlist_path).unwrap();
        writeln!(file, "track1.mp3").unwrap();
        writeln!(file, "track2.mp3").unwrap();
        writeln!(file, "track3.mp3").unwrap();

        let result = M3uParser::parse(&playlist_path).unwrap();

        assert_eq!(result.len(), 3);
        assert_eq!(result[0], tracks[0]);
        assert_eq!(result[1], tracks[1]);
        assert_eq!(result[2], tracks[2]);
    }

    #[test]
    fn given_extended_m3u_with_metadata_when_parsed_then_returns_tracks_ignoring_metadata() {
        let temp_dir = TempDir::new().unwrap();
        let tracks = given_test_tracks_in_directory(temp_dir.path(), 2);

        let playlist_path = temp_dir.path().join("test.m3u");
        let mut file = File::create(&playlist_path).unwrap();
        writeln!(file, "#EXTM3U").unwrap();
        writeln!(file, "#EXTINF:123,Artist - Title 1").unwrap();
        writeln!(file, "track1.mp3").unwrap();
        writeln!(file, "#EXTINF:234,Artist - Title 2").unwrap();
        writeln!(file, "track2.mp3").unwrap();

        let result = M3uParser::parse(&playlist_path).unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0], tracks[0]);
        assert_eq!(result[1], tracks[1]);
    }

    #[test]
    fn given_m3u_with_absolute_paths_when_parsed_then_uses_absolute_paths() {
        let temp_dir = TempDir::new().unwrap();
        let tracks = given_test_tracks_in_directory(temp_dir.path(), 2);

        let playlist_path = temp_dir.path().join("test.m3u");
        let mut file = File::create(&playlist_path).unwrap();
        writeln!(file, "{}", tracks[0].display()).unwrap();
        writeln!(file, "{}", tracks[1].display()).unwrap();

        let result = M3uParser::parse(&playlist_path).unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0], tracks[0]);
        assert_eq!(result[1], tracks[1]);
    }

    #[test]
    fn given_m3u_with_empty_lines_when_parsed_then_skips_empty_lines() {
        let temp_dir = TempDir::new().unwrap();
        given_test_tracks_in_directory(temp_dir.path(), 2);

        let playlist_path = temp_dir.path().join("test.m3u");
        let mut file = File::create(&playlist_path).unwrap();
        writeln!(file).unwrap();
        writeln!(file, "track1.mp3").unwrap();
        writeln!(file).unwrap();
        writeln!(file, "track2.mp3").unwrap();
        writeln!(file).unwrap();

        let result = M3uParser::parse(&playlist_path).unwrap();

        assert_eq!(result.len(), 2);
    }

    #[test]
    fn given_m3u_with_missing_files_when_parsed_then_skips_missing_files_and_logs_warning() {
        let temp_dir = TempDir::new().unwrap();
        given_test_tracks_in_directory(temp_dir.path(), 1);

        let playlist_path = temp_dir.path().join("test.m3u");
        let mut file = File::create(&playlist_path).unwrap();
        writeln!(file, "track1.mp3").unwrap();
        writeln!(file, "missing.mp3").unwrap();

        let result = M3uParser::parse(&playlist_path).unwrap();

        assert_eq!(result.len(), 1);
    }

    #[test]
    fn given_nonexistent_playlist_when_parsed_then_returns_error_with_clear_message() {
        let result = M3uParser::parse(Path::new("/nonexistent/playlist.m3u"));

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("M3U playlist not found"));
    }

    #[test]
    fn given_empty_playlist_file_when_parsed_then_returns_error_about_no_tracks() {
        let temp_dir = TempDir::new().unwrap();
        let playlist_path = temp_dir.path().join("empty.m3u");
        File::create(&playlist_path).unwrap();

        let result = M3uParser::parse(&playlist_path);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No valid tracks found"));
    }

    #[test]
    fn given_m3u_with_only_comments_when_parsed_then_returns_error_about_no_tracks() {
        let temp_dir = TempDir::new().unwrap();
        let playlist_path = temp_dir.path().join("comments.m3u");
        let mut file = File::create(&playlist_path).unwrap();
        writeln!(file, "#EXTM3U").unwrap();
        writeln!(file, "# This is a comment").unwrap();

        let result = M3uParser::parse(&playlist_path);

        assert!(result.is_err());
    }

    #[test]
    fn given_valid_playlist_when_validated_then_returns_track_count() {
        let temp_dir = TempDir::new().unwrap();
        given_test_tracks_in_directory(temp_dir.path(), 3);

        let playlist_path = temp_dir.path().join("test.m3u");
        let mut file = File::create(&playlist_path).unwrap();
        writeln!(file, "track1.mp3").unwrap();
        writeln!(file, "track2.mp3").unwrap();
        writeln!(file, "track3.mp3").unwrap();

        let count = M3uParser::validate_playlist(&playlist_path).unwrap();

        assert_eq!(count, 3);
    }
}
