use crate::audio_metadata::TrackMetadata;
use crate::hearthis_client::{HearthisClient, HearthisTrack};
use crate::library_db::LibraryDatabase;
use crate::schedule_engine::PlaylistCommand;
use chrono::Duration;
use crossbeam_channel::{bounded, Receiver};
use log::{error, info};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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

fn shuffle_playlist(playlist: &mut VecDeque<PathBuf>) {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    std::time::SystemTime::now().hash(&mut hasher);
    let seed = hasher.finish() as usize;

    let mut playlist_vec: Vec<_> = playlist.drain(..).collect();
    for i in (1..playlist_vec.len()).rev() {
        let j = (seed + i * 17) % (i + 1);
        playlist_vec.swap(i, j);
    }
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

        let mut playlist: VecDeque<PathBuf> = tracks
            .into_iter()
            .map(|t| PathBuf::from(t.file_path))
            .collect();

        if shuffle {
            shuffle_playlist(&mut playlist);
        }

        Ok(Self {
            library_shuffle: shuffle,
            library_repeat: repeat,
            playlist,
            current_index: 0,
            current_metadata: Arc::new(Mutex::new(TrackMetadata::default())),
            playlist_source: PlaylistSource::Library,
            db,
        })
    }

    pub fn get_current_metadata(&self) -> Arc<Mutex<TrackMetadata>> {
        Arc::clone(&self.current_metadata)
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
                            shuffle_playlist(&mut self.playlist);
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
                        shuffle_playlist(&mut self.playlist);
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
                    info!("Next track: {:?}", track);

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
