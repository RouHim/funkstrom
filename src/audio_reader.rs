use crate::audio_metadata::TrackMetadata;
use crate::library_db::LibraryDatabase;
use crate::schedule_engine::PlaylistCommand;
use chrono::Duration;
use crossbeam_channel::{unbounded, Receiver};
use log::{error, info};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
enum PlaylistSource {
    Library,
    Scheduled {
        #[allow(dead_code)]
        name: String,
        end_time: std::time::Instant,
    },
}

pub struct AudioReader {
    music_directory: PathBuf,
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
        music_directory: PathBuf,
        shuffle: bool,
        repeat: bool,
        db: LibraryDatabase,
    ) -> Result<Self, Box<dyn std::error::Error>> {
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
            playlist = playlist_vec.into_iter().collect();
        }

        Ok(Self {
            music_directory,
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
                            let mut playlist_vec: Vec<_> = self.playlist.drain(..).collect();

                            use std::collections::hash_map::DefaultHasher;
                            use std::hash::{Hash, Hasher};

                            let mut hasher = DefaultHasher::new();
                            std::time::SystemTime::now().hash(&mut hasher);
                            let seed = hasher.finish() as usize;

                            for i in (1..playlist_vec.len()).rev() {
                                let j = (seed + i * 17) % (i + 1);
                                playlist_vec.swap(i, j);
                            }

                            self.playlist = playlist_vec.into_iter().collect();
                        }
                    } else {
                        return None;
                    }
                }
                PlaylistSource::Scheduled { name: _, end_time } => {
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

        self.playlist_source = PlaylistSource::Scheduled { name, end_time };
    }

    pub fn return_to_library(&mut self) {
        info!("Returning to library playlist");
        self.playlist.clear();

        match self.db.get_all_tracks() {
            Ok(tracks) => {
                if !tracks.is_empty() {
                    let mut new_playlist: VecDeque<PathBuf> = tracks
                        .into_iter()
                        .map(|t| PathBuf::from(t.file_path))
                        .collect();

                    if self.library_shuffle {
                        use std::collections::hash_map::DefaultHasher;
                        use std::hash::{Hash, Hasher};

                        let mut hasher = DefaultHasher::new();
                        std::time::SystemTime::now().hash(&mut hasher);
                        let seed = hasher.finish() as usize;

                        let mut playlist_vec: Vec<_> = new_playlist.drain(..).collect();
                        for i in (1..playlist_vec.len()).rev() {
                            let j = (seed + i * 17) % (i + 1);
                            playlist_vec.swap(i, j);
                        }
                        new_playlist = playlist_vec.into_iter().collect();
                    }

                    self.playlist = new_playlist;
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
        let (track_tx, track_rx) = unbounded::<PathBuf>();

        tokio::spawn(async move {
            loop {
                if let Some(ref cmd_rx) = schedule_command_rx {
                    match cmd_rx.try_recv() {
                        Ok(PlaylistCommand::SwitchToPlaylist {
                            name,
                            tracks,
                            duration,
                        }) => {
                            self.switch_to_scheduled_playlist(name, tracks, duration);
                        }
                        Ok(PlaylistCommand::ReturnToLibrary) => {
                            self.return_to_library();
                        }
                        Err(_) => {}
                    }
                }

                if let Some(track) = self.next_track() {
                    info!("Next track: {:?}", track);
                    if track_tx.send(track).is_err() {
                        error!("Failed to send track to channel");
                        break;
                    }
                } else {
                    info!("End of playlist reached");
                    if !self.library_repeat
                        && matches!(self.playlist_source, PlaylistSource::Library)
                    {
                        break;
                    }
                }

                // Small delay to avoid busy waiting
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        });

        track_rx
    }
}
