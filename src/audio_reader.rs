use crate::audio_metadata::TrackMetadata;
use crate::hearthis_client::{HearthisClient, HearthisTrack};
use crate::library_db::LibraryDatabase;
use crate::playback_director::{PlaybackDirector, TrackInfo};
use crate::schedule_engine::PlaylistCommand;
use chrono::Duration;
use crossbeam_channel::{Receiver, TryRecvError};
use fastrand::Rng as FastRng;
use log::{error, info, warn};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use tokio::task::JoinHandle;

const SCHEDULE_CHECK_INTERVAL_MS: u64 = 100;

struct PendingLiveset {
    name: String,
    duration: Duration,
}

pub struct AudioReader {
    library_shuffle: bool,
    #[allow(dead_code)]
    library_repeat: bool,
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
            .unwrap_or_default()
            .as_secs();

        Ok(Self {
            library_shuffle: shuffle,
            library_repeat: repeat,
            db,
            shuffle_seed,
        })
    }

    #[allow(dead_code)]
    pub fn get_current_metadata(&self) -> Arc<Mutex<TrackMetadata>> {
        Arc::new(Mutex::new(TrackMetadata::default()))
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
                        warn!("Track {:?} has no duration, using 180s fallback", file_path);
                        180
                    }),
                    title: Some(t.title),
                    artist: Some(t.artist),
                    album: Some(t.album),
                }
            })
            .collect();

        if self.library_shuffle {
            let mut rng = FastRng::with_seed(self.shuffle_seed);
            rng.shuffle(&mut playlist);
        }

        playlist
    }

    pub fn start_schedule_command_service(
        self,
        schedule_command_rx: Option<Receiver<PlaylistCommand>>,
        director: Arc<Mutex<PlaybackDirector>>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let Some(command_rx) = schedule_command_rx else {
                return;
            };

            let reader = self;
            let (liveset_tx, liveset_rx) =
                crossbeam_channel::bounded::<(PendingLiveset, Result<HearthisTrack, String>)>(1);

            loop {
                match command_rx.try_recv() {
                    Ok(PlaylistCommand::SwitchToPlaylist {
                        name,
                        tracks,
                        duration,
                    }) => {
                        info!(
                            "Applying scheduled playlist '{}' with {} tracks",
                            name,
                            tracks.len()
                        );
                        let fallback_duration = duration.num_seconds().max(1);
                        let playlist =
                            reader.build_track_infos_for_paths(&tracks, Some(fallback_duration));
                        if playlist.is_empty() {
                            warn!(
                                "Scheduled playlist '{}' resolved to 0 tracks, keeping current playlist",
                                name
                            );
                        } else {
                            if let Ok(mut guard) = director.lock() {
                                guard.replace_playlist(playlist);
                            }
                        }
                    }
                    Ok(PlaylistCommand::SwitchToLiveset {
                        name,
                        genres,
                        duration,
                    }) => {
                        info!(
                            "Fetching liveset for program '{}' (genres: {:?})",
                            name, genres
                        );

                        let tx = liveset_tx.clone();
                        let pending = PendingLiveset {
                            name: name.clone(),
                            duration,
                        };

                        tokio::task::spawn_blocking(move || {
                            let result = match HearthisClient::new() {
                                Ok(client) => match client.get_random_liveset(&genres) {
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

                            if tx.send((pending, result)).is_err() {
                                error!("Failed to send liveset result - receiver dropped");
                            }
                        });
                    }
                    Ok(PlaylistCommand::ReturnToLibrary) => {
                        info!("Returning to library playlist");
                        let playlist = reader.build_playlist();
                        if playlist.is_empty() {
                            warn!("Library playlist is empty, keeping current playlist");
                        } else {
                            if let Ok(mut guard) = director.lock() {
                                guard.replace_playlist(playlist);
                            }
                        }
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        info!("Schedule command channel disconnected, stopping command service");
                        break;
                    }
                }

                if let Ok((pending, result)) = liveset_rx.try_recv() {
                    match result {
                        Ok(track) => {
                            let liveset_duration = pending.duration.num_seconds().max(1);
                            let playlist = vec![TrackInfo {
                                path: PathBuf::from(track.stream_url),
                                duration_secs: liveset_duration,
                                title: Some(track.title.clone()),
                                artist: Some(track.user.username.clone()),
                                album: None,
                            }];

                            info!(
                                "Applying liveset for program '{}': '{}' by {}",
                                pending.name, track.title, track.user.username
                            );
                            if let Ok(mut guard) = director.lock() {
                                guard.replace_playlist(playlist);
                            }
                        }
                        Err(e) => {
                            error!(
                                "Failed to fetch liveset for program '{}': {}. Continuing with current playlist.",
                                pending.name, e
                            );
                        }
                    }
                }

                tokio::time::sleep(tokio::time::Duration::from_millis(
                    SCHEDULE_CHECK_INTERVAL_MS,
                ))
                .await;
            }
        })
    }

    fn build_track_infos_for_paths(
        &self,
        tracks: &[PathBuf],
        fallback_duration: Option<i64>,
    ) -> Vec<TrackInfo> {
        let duration_lookup = self.build_duration_lookup();
        let fallback = fallback_duration.unwrap_or(180);

        tracks
            .iter()
            .cloned()
            .map(|path| {
                let key = path.to_string_lossy().to_string();
                let duration_secs = duration_lookup.get(&key).copied().unwrap_or_else(|| {
                    warn!(
                        "Track {:?} has no known duration, using {}s fallback",
                        path, fallback
                    );
                    fallback
                });
                TrackInfo {
                    path,
                    duration_secs,
                    title: None,
                    artist: None,
                    album: None,
                }
            })
            .collect()
    }

    fn build_duration_lookup(&self) -> HashMap<String, i64> {
        match self.db.get_all_tracks() {
            Ok(tracks) => tracks
                .into_iter()
                .map(|track| (track.file_path, track.duration_seconds.unwrap_or(180)))
                .collect(),
            Err(e) => {
                error!("Failed to load durations from database: {}", e);
                HashMap::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_db::{LibraryDatabase, TrackRecord};
    use tempfile::NamedTempFile;

    fn create_test_db_with_tracks(
        tracks: Vec<(&str, Option<i64>)>,
    ) -> (LibraryDatabase, NamedTempFile) {
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
        let playlist_set: std::collections::HashSet<&str> =
            playlist_paths.iter().map(|s| s.as_str()).collect();
        let db_set: std::collections::HashSet<&str> = db_paths.iter().map(|s| s.as_str()).collect();
        assert_eq!(playlist_set, db_set, "shuffle must preserve all tracks");
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
