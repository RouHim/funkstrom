use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    pub server: ServerConfig,
    pub library: LibraryConfig,
    pub station: StationConfig,
    pub stream: StreamConfig,
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

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct StreamConfig {
    pub bitrate: u32,
    pub format: String,
    pub sample_rate: u32,
    pub channels: u8,
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
    pub playlist: String,
}

impl Config {
    pub fn from_file(path: &PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let content = fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }
}

impl Default for Config {
    fn default() -> Self {
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
            stream: StreamConfig {
                bitrate: 128,
                format: "mp3".to_string(),
                sample_rate: 44100,
                channels: 2,
            },
            schedule: None,
        }
    }
}
