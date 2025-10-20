use log::{debug, error, info};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

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
    client: reqwest::Client,
}

impl HearthisClient {
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        Ok(Self { client })
    }

    /// Fetches a random liveset from the specified genres
    /// If genres is empty, fetches from all available tracks
    pub async fn get_random_liveset(
        &self,
        genres: &[String],
    ) -> Result<HearthisTrack, Box<dyn std::error::Error + Send + Sync>> {
        if genres.is_empty() {
            // Fetch from general feed
            self.fetch_random_from_feed().await
        } else {
            // Try each genre until we find one with tracks
            self.fetch_random_from_genres(genres).await
        }
    }

    async fn fetch_random_from_feed(&self) -> Result<HearthisTrack, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/feed/?page=1&count=20", HEARTHIS_API_BASE);

        debug!("Fetching tracks from feed: {}", url);

        let tracks: Vec<HearthisTrack> = self
            .client
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

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

    async fn fetch_random_from_genres(
        &self,
        genres: &[String],
    ) -> Result<HearthisTrack, Box<dyn std::error::Error + Send + Sync>> {
        // Try each genre in the list
        for genre in genres {
            match self.fetch_from_genre(genre).await {
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
        self.fetch_random_from_feed().await
    }

    async fn fetch_from_genre(
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

        let tracks: Vec<HearthisTrack> = self
            .client
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        if tracks.is_empty() {
            return Err(format!("No tracks found in genre '{}'", genre).into());
        }

        Ok(Self::select_random_track(&tracks))
    }

    fn select_random_track(tracks: &[HearthisTrack]) -> HearthisTrack {
        // Use a simple deterministic random selection based on current time
        let mut hasher = DefaultHasher::new();
        std::time::SystemTime::now().hash(&mut hasher);
        let seed = hasher.finish() as usize;

        let index = seed % tracks.len();
        tracks[index].clone()
    }
}

impl Default for HearthisClient {
    fn default() -> Self {
        Self::new().expect("Failed to create HearthisClient")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_random_track() {
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

    #[tokio::test]
    async fn test_fetch_from_feed_returns_track() {
        let client = HearthisClient::new().unwrap();

        let result = client.fetch_random_from_feed().await;

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

    #[tokio::test]
    async fn test_fetch_from_genre_techno() {
        let client = HearthisClient::new().unwrap();

        let result = client.fetch_from_genre("techno").await;

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

    #[tokio::test]
    async fn test_get_random_liveset_with_genres() {
        let client = HearthisClient::new().unwrap();
        let genres = vec!["techno".to_string(), "house".to_string()];

        let result = client.get_random_liveset(&genres).await;

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

    #[tokio::test]
    async fn test_get_random_liveset_empty_genres() {
        let client = HearthisClient::new().unwrap();
        let genres: Vec<String> = vec![];

        let result = client.get_random_liveset(&genres).await;

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
