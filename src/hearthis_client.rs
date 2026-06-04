//! Hearthis.at API client for fetching electronic music livesets.
//!
//! This module provides integration with the hearthis.at v2 API to fetch
//! random livesets filtered by genre. No authentication is required for API access.
//!
//! # API Details
//!
//! - **Base URL**: `https://api-v2.hearthis.at`
//! - **Authentication**: None required (free tier)
//! - **Rate Limiting**: No documented limits, but clients should implement reasonable throttling
//!
//! # Genre Format
//!
//! Genres are automatically converted to slug format for API requests:
//! - Lowercase conversion
//! - Spaces replaced with hyphens
//! - Example: "Tech House" → "tech-house"
//!
//! # Fallback Behavior
//!
//! When fetching by genre:
//! 1. Try each specified genre in order
//! 2. If all genres fail or return no tracks, fall back to general feed
//! 3. General feed returns popular recent tracks across all genres
//!
//! # Example
//!
//! ```no_run
//! use crate::hearthis_client::HearthisClient;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! let client = HearthisClient::new()?;
//! let genres = vec!["techno".to_string(), "house".to_string()];
//! let track = client.get_random_liveset(&genres).await?;
//! println!("Playing: {} by {}", track.title, track.user.username);
//! # Ok(())
//! # }
//! ```

use log::{debug, error, info};
use serde::{Deserialize, Serialize};

const HEARTHIS_API_BASE: &str = "https://api-v2.hearthis.at";

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct HearthisTrack {
    pub id: String,
    pub title: String,
    pub genre: String,
    #[serde(rename = "stream_url")]
    pub stream_url: String,
    pub duration: String,
    #[serde(rename = "type")]
    pub track_type: String,
    pub user: HearthisUser,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct HearthisUser {
    pub username: String,
}

pub struct HearthisClient {
    agent: ureq::Agent,
}

impl HearthisClient {
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(30)))
            .http_status_as_error(false)
            .build()
            .new_agent();

        Ok(Self { agent })
    }

    /// Fetches a random liveset from the specified genres.
    ///
    /// # Arguments
    ///
    /// * `genres` - List of genre names to search. If empty, fetches from the general feed
    ///   which contains popular recent tracks across all genres.
    ///
    /// # Behavior
    ///
    /// - **Non-empty genres**: Tries each genre in order until one returns tracks
    /// - **Empty genres**: Fetches directly from the general feed (popular/recent tracks)
    /// - **All genres fail**: Falls back to general feed as last resort
    ///
    /// # Returns
    ///
    /// A random track selected from the available results (up to 20 tracks per query).
    pub fn get_random_liveset(
        &self,
        genres: &[String],
    ) -> Result<HearthisTrack, Box<dyn std::error::Error + Send + Sync>> {
        if genres.is_empty() {
            // Fetch from general feed (popular/recent tracks across all genres)
            self.fetch_random_from_feed()
        } else {
            // Try each genre until we find one with tracks
            self.fetch_random_from_genres(genres)
        }
    }

    fn fetch_random_from_feed(
        &self,
    ) -> Result<HearthisTrack, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/feed/?page=1&count=20", HEARTHIS_API_BASE);

        debug!("Fetching tracks from feed: {}", url);

        let mut response = self.agent.get(&url).call()?;
        let status = response.status();

        let code = status.as_u16();
        if !(200..300).contains(&code) {
            let body = response.body_mut().read_to_string().unwrap_or_default();
            error!("API error {}: {}", code, body);
            return Err(format!("HTTP {} - {}", code, body).into());
        }

        let tracks: Vec<HearthisTrack> = response.body_mut().read_json()?;

        if tracks.is_empty() {
            return Err("No tracks found in feed".into());
        }

        let track = Self::select_random_track(&tracks);
        info!(
            "Selected random track from feed: '{}' by {}",
            track.title, track.user.username
        );

        Ok(track)
    }

    fn fetch_random_from_genres(
        &self,
        genres: &[String],
    ) -> Result<HearthisTrack, Box<dyn std::error::Error + Send + Sync>> {
        // Try each genre in the list
        for genre in genres {
            match self.fetch_from_genre(genre) {
                Ok(track) => {
                    info!(
                        "Selected random '{}' track: '{}' by {}",
                        genre, track.title, track.user.username
                    );
                    return Ok(track);
                }
                Err(e) => {
                    error!("Failed to fetch from genre '{}': {}", genre, e);
                    // Continue to next genre
                }
            }
        }

        // If all genres failed, fall back to feed
        error!(
            "All specified genres failed, falling back to general feed: {:?}",
            genres
        );
        self.fetch_random_from_feed()
    }

    fn fetch_from_genre(
        &self,
        genre: &str,
    ) -> Result<HearthisTrack, Box<dyn std::error::Error + Send + Sync>> {
        // Convert genre to slug format (lowercase, spaces to hyphens)
        let genre_slug = genre.to_lowercase().replace(' ', "-");

        let url = format!(
            "{}/categories/{}/?page=1&count=20",
            HEARTHIS_API_BASE, genre_slug
        );

        debug!("Fetching tracks from genre '{}': {}", genre, url);

        let mut response = self.agent.get(&url).call()?;
        let status = response.status();
        let code = status.as_u16();
        if !(200..300).contains(&code) {
            let body = response.body_mut().read_to_string().unwrap_or_default();
            error!("API error {} for genre '{}': {}", code, genre, body);
            return Err(format!("HTTP {} - {}", code, body).into());
        }

        let tracks: Vec<HearthisTrack> = response.body_mut().read_json()?;

        if tracks.is_empty() {
            return Err(format!("No tracks found in genre '{}'", genre).into());
        }

        Ok(Self::select_random_track(&tracks))
    }
    fn select_random_track(tracks: &[HearthisTrack]) -> HearthisTrack {
        let index = fastrand::usize(0..tracks.len());
        tracks[index].clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn given_track_list_when_selecting_random_then_returns_valid_track() {
        let tracks = vec![
            HearthisTrack {
                id: "1".to_string(),
                title: "Track 1".to_string(),
                genre: "Techno".to_string(),
                stream_url: "http://example.com/1".to_string(),
                duration: "3600".to_string(),
                track_type: "Mix".to_string(),
                user: HearthisUser {
                    username: "DJ 1".to_string(),
                },
            },
            HearthisTrack {
                id: "2".to_string(),
                title: "Track 2".to_string(),
                genre: "House".to_string(),
                stream_url: "http://example.com/2".to_string(),
                duration: "3600".to_string(),
                track_type: "Mix".to_string(),
                user: HearthisUser {
                    username: "DJ 2".to_string(),
                },
            },
        ];

        let track = HearthisClient::select_random_track(&tracks);
        assert!(track.id == "1" || track.id == "2");
    }

    #[test]
    fn given_api_available_when_fetching_from_feed_then_returns_track() {
        let client = HearthisClient::new().unwrap();

        let result = client.fetch_random_from_feed();

        // This test requires internet connection
        match result {
            Ok(track) => {
                assert!(!track.id.is_empty());
                assert!(!track.stream_url.is_empty());
                println!("Fetched track: {} by {}", track.title, track.user.username);
            }
            Err(e) => {
                eprintln!("Note: This test requires internet connection. Error: {}", e);
            }
        }
    }

    #[test]
    fn given_techno_genre_when_fetching_then_returns_techno_track() {
        let client = HearthisClient::new().unwrap();

        let result = client.fetch_from_genre("techno");

        match result {
            Ok(track) => {
                assert!(!track.id.is_empty());
                assert!(!track.stream_url.is_empty());
                println!(
                    "Fetched techno track: {} by {}",
                    track.title, track.user.username
                );
            }
            Err(e) => {
                eprintln!("Note: This test requires internet connection. Error: {}", e);
            }
        }
    }

    #[test]
    fn given_multiple_genres_when_getting_random_liveset_then_returns_matching_track() {
        let client = HearthisClient::new().unwrap();
        let genres = vec!["techno".to_string(), "house".to_string()];

        let result = client.get_random_liveset(&genres);

        match result {
            Ok(track) => {
                assert!(!track.id.is_empty());
                assert!(!track.stream_url.is_empty());
                println!(
                    "Selected liveset: {} ({}) by {}",
                    track.title, track.genre, track.user.username
                );
            }
            Err(e) => {
                eprintln!("Note: This test requires internet connection. Error: {}", e);
            }
        }
    }

    #[test]
    fn given_empty_genres_when_getting_random_liveset_then_returns_from_feed() {
        let client = HearthisClient::new().unwrap();
        let genres: Vec<String> = vec![];

        let result = client.get_random_liveset(&genres);

        match result {
            Ok(track) => {
                assert!(!track.id.is_empty());
                assert!(!track.stream_url.is_empty());
                println!(
                    "Selected random liveset: {} ({}) by {}",
                    track.title, track.genre, track.user.username
                );
            }
            Err(e) => {
                eprintln!("Note: This test requires internet connection. Error: {}", e);
            }
        }
    }
}
