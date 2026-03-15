use crate::audio_metadata::TrackMetadata;
use crate::hearthis_client::{HearthisClient, HearthisTrack};
use crate::library_db::LibraryDatabase;
use crate::playback_director::TrackInfo;
use crate::schedule_engine::PlaylistCommand;
use chrono::Duration;
use crossbeam_channel::{bounded, Receiver};
use log::{debug, error, info, warn};
use rand::prelude::*;
use rand::rngs::StdRng;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

// Constants for audio reader configuration
const TRACK_BUFFER_SIZE: usize = 2; // Number of tracks to buffer ahead
const SCHEDULE_CHECK_INTERVAL_MS: u64 = 100; // How often to check for schedule commands

#[derive(Debug, Clone)]
enum PlaylistSource {
    Library,
    Scheduled { end_time: std::time::Instant },
}

// Struct to track pending liveset fetch requests
#[derive(Debug)]
struct PendingLiveset {
    name: String,
    duration: Duration,
}

fn shuffle_playlist(playlist: &mut VecDeque<PathBuf>, seed: u64) {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut playlist_vec: Vec<_> = playlist.drain(..).collect();
    playlist_vec.shuffle(&mut rng);
    *playlist = playlist_vec.into_iter().collect();
}

pub struct AudioReader {
    library_shuffle: bool,
    library_repeat: bool,
    playlist: VecDeque<PathBuf>,
    current_index: usize,
    current_metadata: Arc<Mutex<TrackMetadata>>,
    playlist_source: PlaylistSource,
    db: LibraryDatabase,
    shuffle_seed: u64,
}

impl AudioReader {
    pub fn new(
        _music_directory: PathBuf,
        shuffle: bool,
        repeat: bool,
        db: LibraryDatabase,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let tracks = db.get_all_tracks()?;

        if tracks.is_empty() {
            return Err("No tracks found in library database".into());
        }

        info!("Loaded {} tracks from database", tracks.len());

        let shuffle_seed = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut playlist: VecDeque<PathBuf> = tracks
            .into_iter()
            .map(|t| PathBuf::from(t.file_path))
            .collect();

        if shuffle {
            shuffle_playlist(&mut playlist, shuffle_seed);
        }

        Ok(Self {
            library_shuffle: shuffle,
            library_repeat: repeat,
            playlist,
            current_index: 0,
            current_metadata: Arc::new(Mutex::new(TrackMetadata::default())),
            playlist_source: PlaylistSource::Library,
            db,
            shuffle_seed,
        })
    }

    pub fn get_current_metadata(&self) -> Arc<Mutex<TrackMetadata>> {
        Arc::clone(&self.current_metadata)
    }

    #[allow(dead_code)]
    pub fn build_playlist(&self) -> Vec<TrackInfo> {
        let tracks = match self.db.get_all_tracks() {
            Ok(tracks) => tracks,
            Err(e) => {
                error!("Failed to get tracks from database: {}", e);
                return Vec::new();
            }
        };

        let mut playlist: Vec<TrackInfo> = tracks
            .into_iter()
            .map(|t| {
                let file_path = t.file_path.clone();
                TrackInfo {
                    path: PathBuf::from(t.file_path),
                    duration_secs: t.duration_seconds.unwrap_or_else(|| {
                        warn!(
                            "Track {:?} has no duration, using 180s fallback",
                            file_path
                        );
                        180
                    }),
                }
            })
            .collect();

        if self.library_shuffle {
            let mut rng = StdRng::seed_from_u64(self.shuffle_seed);
            playlist.shuffle(&mut rng);
        }

        playlist
    }

    pub fn next_track(&mut self) -> Option<PathBuf> {
        if self.playlist.is_empty() {
            return None;
        }

        let track = self.playlist.get(self.current_index).cloned();

        // Extract and store metadata for current track
        if let Some(ref track_path) = track {
            let metadata = TrackMetadata::from_file(track_path);
            if let Ok(mut current) = self.current_metadata.lock() {
                *current = metadata;
            }
        }

        self.current_index += 1;

        if self.current_index >= self.playlist.len() {
            match &self.playlist_source {
                PlaylistSource::Library => {
                    if self.library_repeat {
                        self.current_index = 0;
                        if self.library_shuffle {
                            shuffle_playlist(&mut self.playlist, self.shuffle_seed);
                        }
                    } else {
                        return None;
                    }
                }
                PlaylistSource::Scheduled { end_time } => {
                    if std::time::Instant::now() >= *end_time {
                        info!("Scheduled program ended, returning to library");
                        self.return_to_library();
                        return self.next_track();
                    } else {
                        self.current_index = 0;
                    }
                }
            }
        }

        track
    }

    pub fn switch_to_scheduled_playlist(
        &mut self,
        name: String,
        tracks: Vec<PathBuf>,
        duration: Duration,
    ) {
        info!(
            "Switching to scheduled playlist '{}' with {} tracks",
            name,
            tracks.len()
        );

        self.playlist = tracks.into_iter().collect();
        self.current_index = 0;

        let duration_std = std::time::Duration::from_secs(duration.num_seconds() as u64);
        let end_time = std::time::Instant::now() + duration_std;

        self.playlist_source = PlaylistSource::Scheduled { end_time };
    }

    pub fn return_to_library(&mut self) {
        info!("Returning to library playlist");
        self.playlist.clear();

        match self.db.get_all_tracks() {
            Ok(tracks) => {
                if !tracks.is_empty() {
                    self.playlist = tracks
                        .into_iter()
                        .map(|t| PathBuf::from(t.file_path))
                        .collect();

                    if self.library_shuffle {
                        shuffle_playlist(&mut self.playlist, self.shuffle_seed);
                    }

                    self.current_index = 0;
                    self.playlist_source = PlaylistSource::Library;
                } else {
                    error!("No tracks found in database when returning to library");
                }
            }
            Err(e) => {
                error!("Failed to load tracks from database: {}", e);
            }
        }
    }

    pub fn start_playlist_service(
        mut self,
        schedule_command_rx: Option<Receiver<PlaylistCommand>>,
    ) -> Receiver<PathBuf> {
        // Use bounded channel to keep tracks buffered ahead
        // This provides backpressure and prevents flooding the channel
        let (track_tx, track_rx) = bounded::<PathBuf>(TRACK_BUFFER_SIZE);

        // Channel for receiving fetched livesets from async tasks
        let (liveset_tx, liveset_rx) =
            bounded::<(PendingLiveset, Result<HearthisTrack, String>)>(1);

        tokio::spawn(async move {
            loop {
                // Check for schedule commands
                if let Some(ref cmd_rx) = schedule_command_rx {
                    match cmd_rx.try_recv() {
                        Ok(PlaylistCommand::SwitchToPlaylist {
                            name,
                            tracks,
                            duration,
                        }) => {
                            self.switch_to_scheduled_playlist(name, tracks, duration);
                        }
                        Ok(PlaylistCommand::SwitchToLiveset {
                            name,
                            genres,
                            duration,
                        }) => {
                            // Fetch liveset from hearthis.at API asynchronously
                            info!(
                                "Fetching liveset for program '{}' (genres: {:?})",
                                name, genres
                            );

                            // Spawn async task to fetch liveset and send result back via channel
                            let tx = liveset_tx.clone();
                            let pending = PendingLiveset {
                                name: name.clone(),
                                duration,
                            };

                            tokio::spawn(async move {
                                let result = match HearthisClient::new() {
                                    Ok(client) => match client.get_random_liveset(&genres).await {
                                        Ok(track) => {
                                            info!(
                                                "Fetched liveset: '{}' by {} ({})",
                                                track.title, track.user.username, track.genre
                                            );
                                            Ok(track)
                                        }
                                        Err(e) => {
                                            error!("Failed to fetch liveset: {}", e);
                                            Err(format!("API error: {}", e))
                                        }
                                    },
                                    Err(e) => {
                                        error!("Failed to create hearthis client: {}", e);
                                        Err(format!("Client error: {}", e))
                                    }
                                };

                                // Send result back to main loop
                                if tx.send((pending, result)).is_err() {
                                    error!("Failed to send liveset result - receiver dropped");
                                }
                            });
                        }
                        Ok(PlaylistCommand::ReturnToLibrary) => {
                            self.return_to_library();
                        }
                        Err(_) => {}
                    }
                }

                // Check for liveset fetch results
                if let Ok((pending, result)) = liveset_rx.try_recv() {
                    match result {
                        Ok(track) => {
                            info!(
                                "Liveset fetched successfully for program '{}': '{}' by {}",
                                pending.name, track.title, track.user.username
                            );

                            // Switch to the liveset by treating the stream URL as a track
                            let liveset_url = PathBuf::from(track.stream_url);
                            self.switch_to_scheduled_playlist(
                                pending.name,
                                vec![liveset_url],
                                pending.duration,
                            );
                        }
                        Err(e) => {
                            error!(
                                "Failed to fetch liveset for program '{}': {}. Continuing with library.",
                                pending.name, e
                            );
                            // Continue with library playback on error
                        }
                    }
                }

                // Get next track
                if let Some(track) = self.next_track() {
                    debug!("Next track: {:?}", track);

                    // This will block when channel is full (backpressure)
                    // Blocking is moved to tokio blocking thread to avoid blocking async runtime
                    let result = tokio::task::spawn_blocking({
                        let track_tx = track_tx.clone();
                        let track = track.clone();
                        move || track_tx.send(track)
                    })
                    .await;

                    match result {
                        Ok(Ok(())) => {
                            // Track sent successfully
                        }
                        Ok(Err(_)) => {
                            error!("Failed to send track to channel - receiver dropped");
                            break;
                        }
                        Err(e) => {
                            error!("Task join error: {}", e);
                            break;
                        }
                    }
                } else {
                    info!("End of playlist reached");
                    if !self.library_repeat
                        && matches!(self.playlist_source, PlaylistSource::Library)
                    {
                        break;
                    }
                }

                // Small delay to check for schedule commands periodically
                tokio::time::sleep(tokio::time::Duration::from_millis(
                    SCHEDULE_CHECK_INTERVAL_MS,
                ))
                .await;
            }
        });

        track_rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_db::{LibraryDatabase, TrackRecord};
    use tempfile::NamedTempFile;

    fn create_test_db_with_tracks(tracks: Vec<(&str, Option<i64>)>) -> (LibraryDatabase, NamedTempFile) {
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_str().unwrap();
        let db = LibraryDatabase::new(db_path).unwrap();
        db.initialize_schema().unwrap();

        for (i, (path, duration)) in tracks.iter().enumerate() {
            let track = TrackRecord {
                file_path: path.to_string(),
                title: format!("Track {}", i + 1),
                artist: "Test Artist".to_string(),
                album: "Test Album".to_string(),
                duration_seconds: *duration,
                file_size: 3000000,
                last_modified: 1234567890,
                file_extension: "mp3".to_string(),
                created_at: 1234567890,
                updated_at: 1234567890,
            };
            db.insert_track(&track).unwrap();
        }

        (db, temp_file)
    }

    #[test]
    fn given_same_seed_when_shuffled_twice_then_playlist_order_is_identical() {
        let (db, _temp) = create_test_db_with_tracks(vec![
            ("/music/a.mp3", Some(180)),
            ("/music/b.mp3", Some(200)),
            ("/music/c.mp3", Some(220)),
            ("/music/d.mp3", Some(240)),
        ]);

        let reader1 = AudioReader::new(PathBuf::from("/music"), true, true, db.clone()).unwrap();

        let playlist1 = reader1.build_playlist();
        let playlist2 = reader1.build_playlist();

        assert_eq!(playlist1.len(), 4);
        assert_eq!(playlist2.len(), 4);

        for i in 0..4 {
            assert_eq!(playlist1[i].path, playlist2[i].path);
        }
    }

    #[test]
    fn given_track_without_duration_when_building_playlist_then_uses_180s_fallback() {
        let (db, _temp) = create_test_db_with_tracks(vec![
            ("/music/a.mp3", Some(200)),
            ("/music/b.mp3", None),
            ("/music/c.mp3", Some(300)),
        ]);

        let reader = AudioReader::new(PathBuf::from("/music"), false, true, db).unwrap();
        let playlist = reader.build_playlist();

        assert_eq!(playlist.len(), 3);
        assert_eq!(playlist[0].duration_secs, 200);
        assert_eq!(playlist[1].duration_secs, 180);
        assert_eq!(playlist[2].duration_secs, 300);
    }

    #[test]
    fn given_shuffle_enabled_when_building_playlist_then_order_differs_from_database_order() {
        let (db, _temp) = create_test_db_with_tracks(vec![
            ("/music/a.mp3", Some(180)),
            ("/music/b.mp3", Some(200)),
            ("/music/c.mp3", Some(220)),
            ("/music/d.mp3", Some(240)),
            ("/music/e.mp3", Some(260)),
        ]);

        let reader = AudioReader::new(PathBuf::from("/music"), true, true, db.clone()).unwrap();
        let playlist = reader.build_playlist();

        let db_tracks = db.get_all_tracks().unwrap();
        let db_paths: Vec<String> = db_tracks.iter().map(|t| t.file_path.clone()).collect();
        let playlist_paths: Vec<String> = playlist
            .iter()
            .map(|t| t.path.to_str().unwrap().to_string())
            .collect();

        assert_ne!(db_paths, playlist_paths);
    }

    #[test]
    fn given_shuffle_disabled_when_building_playlist_then_order_matches_database_order() {
        let (db, _temp) = create_test_db_with_tracks(vec![
            ("/music/a.mp3", Some(180)),
            ("/music/b.mp3", Some(200)),
            ("/music/c.mp3", Some(220)),
        ]);

        let reader = AudioReader::new(PathBuf::from("/music"), false, true, db.clone()).unwrap();
        let playlist = reader.build_playlist();

        let db_tracks = db.get_all_tracks().unwrap();

        assert_eq!(playlist.len(), 3);
        for (i, track) in playlist.iter().enumerate() {
            assert_eq!(track.path.to_str().unwrap(), db_tracks[i].file_path);
        }
    }
}

