use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    pub server: ServerConfig,
    pub audio: AudioConfig,
    pub stream: StreamConfig,
    pub ffmpeg: FFmpegConfig,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub bind_address: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AudioConfig {
    pub music_directory: String,
    pub shuffle: bool,
    pub repeat: bool,
    pub bitrate: u32,
    pub format: String,
    pub sample_rate: u32,
    pub channels: u8,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct StreamConfig {
    pub station_name: String,
    pub description: String,
    pub genre: String,
    pub url: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct FFmpegConfig {
    pub path: Option<String>,
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
                port: 8000,
                bind_address: "127.0.0.1".to_string(),
            },
            audio: AudioConfig {
                music_directory: "/path/to/music".to_string(),
                shuffle: true,
                repeat: true,
                bitrate: 128,
                format: "mp3".to_string(),
                sample_rate: 44100,
                channels: 2,
            },
            stream: StreamConfig {
                station_name: "My Radio Station".to_string(),
                description: "Great music 24/7".to_string(),
                genre: "Various".to_string(),
                url: "http://localhost:8000".to_string(),
            },
            ffmpeg: FFmpegConfig { path: None },
        }
    }
}
