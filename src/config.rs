use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    pub server: ServerConfig,
    pub library: LibraryConfig,
    pub station: StationConfig,
    pub stream: HashMap<String, StreamConfig>,
    pub schedule: Option<ScheduleConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub bind_address: String,
    pub ffmpeg_path: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LibraryConfig {
    pub music_directory: String,
    pub shuffle: bool,
    pub repeat: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct StationConfig {
    pub station_name: String,
    pub description: String,
    pub genre: String,
    pub url: String,
}

/// Configuration for an individual audio stream.
///
/// Supported formats: mp3, aac, opus, ogg
/// Stream names must contain only alphanumeric characters, underscores, or hyphens
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct StreamConfig {
    pub bitrate: u32,
    pub format: String,
    pub sample_rate: u32,
    pub channels: u8,
    pub enabled: bool,
}

impl StreamConfig {
    /// Validates the stream configuration
    pub fn validate(&self) -> Result<(), String> {
        // Validate format
        match self.format.to_lowercase().as_str() {
            "mp3" | "aac" | "opus" | "ogg" => {}
            _ => {
                return Err(format!(
                    "Unsupported audio format '{}'. Supported formats: mp3, aac, opus, ogg",
                    self.format
                ))
            }
        }

        // Validate bitrate
        if self.bitrate < 32 || self.bitrate > 320 {
            return Err(format!(
                "Bitrate {} is out of range. Valid range: 32-320 kbps",
                self.bitrate
            ));
        }

        // Validate sample rate
        match self.sample_rate {
            8000 | 11025 | 16000 | 22050 | 32000 | 44100 | 48000 => {},
            _ => return Err(format!("Unsupported sample rate {}. Valid rates: 8000, 11025, 16000, 22050, 32000, 44100, 48000", self.sample_rate))
        }

        // Validate channels
        if self.channels != 1 && self.channels != 2 {
            return Err(format!(
                "Invalid channel count {}. Valid values: 1 (mono) or 2 (stereo)",
                self.channels
            ));
        }

        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ScheduleConfig {
    pub programs: Vec<ScheduleProgram>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ScheduleProgram {
    pub name: String,
    pub active: bool,
    pub cron: String,
    pub duration: String,
    #[serde(rename = "type")]
    pub program_type: Option<String>,
    pub playlist: Option<String>,
    pub genres: Option<Vec<String>>,
}

impl ScheduleProgram {
    /// Returns the program type, defaulting to "playlist" if not specified
    pub fn get_type(&self) -> ProgramType {
        match self.program_type.as_deref() {
            Some("liveset") => ProgramType::Liveset,
            _ => {
                // Default to playlist if type is not specified or is "playlist"
                ProgramType::Playlist
            }
        }
    }

    /// Validates the program configuration
    pub fn validate(&self) -> Result<(), String> {
        match self.get_type() {
            ProgramType::Playlist => {
                if self.playlist.is_none() {
                    return Err("Playlist programs must specify a 'playlist' field".to_string());
                }
            }
            ProgramType::Liveset => {
                if self.genres.is_none() {
                    return Err(
                        "Liveset programs must specify a 'genres' field (use empty array [] for all genres)"
                            .to_string(),
                    );
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProgramType {
    Playlist,
    Liveset,
}

impl Config {
    pub fn from_file(path: &PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let content = fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;

        // Validate stream configuration
        config.validate()?;

        Ok(config)
    }

    /// Validates the entire configuration
    fn validate(&self) -> Result<(), Box<dyn std::error::Error>> {
        // Check for empty stream configuration
        if self.stream.is_empty() {
            return Err("No streams configured. At least one stream must be defined in [stream.NAME] section".into());
        }

        // Validate stream names and configurations
        for (name, stream_config) in &self.stream {
            // Validate stream name
            if name.is_empty() {
                return Err("Stream name cannot be empty".into());
            }

            if !name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
            {
                return Err(format!(
                    "Invalid stream name '{}'. Stream names must contain only alphanumeric characters, underscores, or hyphens",
                    name
                ).into());
            }

            // Validate stream configuration
            stream_config
                .validate()
                .map_err(|e| format!("Stream '{}': {}", name, e))?;
        }

        // Check that at least one stream is enabled
        if !self.stream.values().any(|s| s.enabled) {
            return Err("At least one stream must be enabled".into());
        }

        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        let mut streams = HashMap::new();
        streams.insert(
            "default".to_string(),
            StreamConfig {
                bitrate: 128,
                format: "mp3".to_string(),
                sample_rate: 44100,
                channels: 2,
                enabled: true,
            },
        );

        Self {
            server: ServerConfig {
                port: 8284,
                bind_address: "127.0.0.1".to_string(),
                ffmpeg_path: None,
            },
            library: LibraryConfig {
                music_directory: "/path/to/music".to_string(),
                shuffle: true,
                repeat: true,
            },
            station: StationConfig {
                station_name: "My Radio Station".to_string(),
                description: "Great music 24/7".to_string(),
                genre: "Various".to_string(),
                url: "http://localhost:8000".to_string(),
            },
            stream: streams,
            schedule: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_stream_config_validation_valid() {
        let config = StreamConfig {
            bitrate: 128,
            format: "mp3".to_string(),
            sample_rate: 44100,
            channels: 2,
            enabled: true,
        };

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_stream_config_validation_invalid_format() {
        let config = StreamConfig {
            bitrate: 128,
            format: "flac".to_string(),
            sample_rate: 44100,
            channels: 2,
            enabled: true,
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unsupported audio format"));
    }

    #[test]
    fn test_stream_config_validation_invalid_bitrate() {
        let config = StreamConfig {
            bitrate: 512,
            format: "mp3".to_string(),
            sample_rate: 44100,
            channels: 2,
            enabled: true,
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("out of range"));
    }

    #[test]
    fn test_stream_config_validation_invalid_sample_rate() {
        let config = StreamConfig {
            bitrate: 128,
            format: "mp3".to_string(),
            sample_rate: 99999,
            channels: 2,
            enabled: true,
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unsupported sample rate"));
    }

    #[test]
    fn test_stream_config_validation_invalid_channels() {
        let config = StreamConfig {
            bitrate: 128,
            format: "mp3".to_string(),
            sample_rate: 44100,
            channels: 5,
            enabled: true,
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid channel count"));
    }

    #[test]
    fn test_config_validation_empty_streams() {
        let mut config = Config::default();
        config.stream.clear();

        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No streams configured"));
    }

    #[test]
    fn test_config_validation_invalid_stream_name() {
        let mut config = Config::default();
        config.stream.clear();
        config.stream.insert(
            "test@stream".to_string(),
            StreamConfig {
                bitrate: 128,
                format: "mp3".to_string(),
                sample_rate: 44100,
                channels: 2,
                enabled: true,
            },
        );

        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid stream name"));
    }

    #[test]
    fn test_config_validation_all_streams_disabled() {
        let mut config = Config::default();
        for stream in config.stream.values_mut() {
            stream.enabled = false;
        }

        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("At least one stream must be enabled"));
    }

    #[test]
    fn test_config_from_file_valid() {
        let toml_content = r#"
[server]
port = 8284
bind_address = "0.0.0.0"

[library]
music_directory = "/music"
shuffle = true
repeat = true

[station]
station_name = "Test Radio"
description = "Test Description"
genre = "Test"
url = "http://test.local"

[stream.high]
bitrate = 320
format = "mp3"
sample_rate = 48000
channels = 2
enabled = true

[stream.low]
bitrate = 64
format = "aac"
sample_rate = 22050
channels = 1
enabled = true
"#;

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(toml_content.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        let config = Config::from_file(&temp_file.path().to_path_buf());
        assert!(config.is_ok());

        let config = config.unwrap();
        assert_eq!(config.stream.len(), 2);
        assert!(config.stream.contains_key("high"));
        assert!(config.stream.contains_key("low"));
    }

    #[test]
    fn test_config_from_file_validation_error() {
        let toml_content = r#"
[server]
port = 8284
bind_address = "0.0.0.0"

[library]
music_directory = "/music"
shuffle = true
repeat = true

[station]
station_name = "Test Radio"
description = "Test Description"
genre = "Test"
url = "http://test.local"

[stream.invalid@name]
bitrate = 128
format = "mp3"
sample_rate = 44100
channels = 2
enabled = true
"#;

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(toml_content.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        let config = Config::from_file(&temp_file.path().to_path_buf());
        assert!(config.is_err());
    }

    #[test]
    fn test_multiple_formats_validation() {
        for format in &["mp3", "aac", "opus", "ogg"] {
            let config = StreamConfig {
                bitrate: 128,
                format: format.to_string(),
                sample_rate: 44100,
                channels: 2,
                enabled: true,
            };
            assert!(
                config.validate().is_ok(),
                "Format {} should be valid",
                format
            );
        }
    }

    #[test]
    fn test_valid_stream_names() {
        let mut config = Config::default();

        // Test valid names
        for name in &["stream1", "high-quality", "low_bitrate", "Stream_123"] {
            config.stream.clear();
            config.stream.insert(
                name.to_string(),
                StreamConfig {
                    bitrate: 128,
                    format: "mp3".to_string(),
                    sample_rate: 44100,
                    channels: 2,
                    enabled: true,
                },
            );
            assert!(
                config.validate().is_ok(),
                "Stream name '{}' should be valid",
                name
            );
        }
    }

    #[test]
    fn test_playlist_program_validation_success() {
        let program = ScheduleProgram {
            name: "test".to_string(),
            active: true,
            cron: "0 0 * * * *".to_string(),
            duration: "30m".to_string(),
            program_type: Some("playlist".to_string()),
            playlist: Some("test.m3u".to_string()),
            genres: None,
        };

        assert!(program.validate().is_ok());
    }

    #[test]
    fn test_playlist_program_validation_missing_playlist() {
        let program = ScheduleProgram {
            name: "test".to_string(),
            active: true,
            cron: "0 0 * * * *".to_string(),
            duration: "30m".to_string(),
            program_type: Some("playlist".to_string()),
            playlist: None,
            genres: None,
        };

        let result = program.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("must specify a 'playlist' field"));
    }

    #[test]
    fn test_liveset_program_validation_success() {
        let program = ScheduleProgram {
            name: "test".to_string(),
            active: true,
            cron: "0 0 * * * *".to_string(),
            duration: "30m".to_string(),
            program_type: Some("liveset".to_string()),
            playlist: None,
            genres: Some(vec!["techno".to_string(), "house".to_string()]),
        };

        assert!(program.validate().is_ok());
    }

    #[test]
    fn test_liveset_program_validation_empty_genres() {
        let program = ScheduleProgram {
            name: "test".to_string(),
            active: true,
            cron: "0 0 * * * *".to_string(),
            duration: "30m".to_string(),
            program_type: Some("liveset".to_string()),
            playlist: None,
            genres: Some(vec![]),
        };

        assert!(program.validate().is_ok());
    }

    #[test]
    fn test_liveset_program_validation_missing_genres() {
        let program = ScheduleProgram {
            name: "test".to_string(),
            active: true,
            cron: "0 0 * * * *".to_string(),
            duration: "30m".to_string(),
            program_type: Some("liveset".to_string()),
            playlist: None,
            genres: None,
        };

        let result = program.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("must specify a 'genres' field"));
    }

    #[test]
    fn test_program_type_defaults_to_playlist() {
        let program = ScheduleProgram {
            name: "test".to_string(),
            active: true,
            cron: "0 0 * * * *".to_string(),
            duration: "30m".to_string(),
            program_type: None,
            playlist: Some("test.m3u".to_string()),
            genres: None,
        };

        assert_eq!(program.get_type(), ProgramType::Playlist);
    }

    #[test]
    fn test_program_type_liveset() {
        let program = ScheduleProgram {
            name: "test".to_string(),
            active: true,
            cron: "0 0 * * * *".to_string(),
            duration: "30m".to_string(),
            program_type: Some("liveset".to_string()),
            playlist: None,
            genres: Some(vec![]),
        };

        assert_eq!(program.get_type(), ProgramType::Liveset);
    }
}
