use crate::audio_metadata::TrackMetadata;
use crate::schedule_engine::PlaylistCommand;
use chrono::Duration;
use crossbeam_channel::{unbounded, Receiver};
use log::{debug, error, info};
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
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
}

impl AudioReader {
    pub fn new(
        music_directory: PathBuf,
        shuffle: bool,
        repeat: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut reader = Self {
            music_directory,
            library_shuffle: shuffle,
            library_repeat: repeat,
            playlist: VecDeque::new(),
            current_index: 0,
            current_metadata: Arc::new(Mutex::new(TrackMetadata::default())),
            playlist_source: PlaylistSource::Library,
        };
        reader.scan_music_directory()?;
        Ok(reader)
    }

    pub fn get_current_metadata(&self) -> Arc<Mutex<TrackMetadata>> {
        Arc::clone(&self.current_metadata)
    }

    fn scan_music_directory(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        info!("Scanning music directory: {:?}", self.music_directory);

        if !self.music_directory.exists() {
            return Err(
                format!("Music directory does not exist: {:?}", self.music_directory).into(),
            );
        }

        let mut files = Vec::new();
        self.scan_directory_recursive(&self.music_directory, &mut files)?;

        info!("Found {} audio files", files.len());

        if files.is_empty() {
            return Err("No audio files found in music directory".into());
        }

        if self.library_shuffle {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};

            let mut hasher = DefaultHasher::new();
            std::time::SystemTime::now().hash(&mut hasher);
            let seed = hasher.finish() as usize;

            for i in (1..files.len()).rev() {
                let j = (seed + i * 17) % (i + 1);
                files.swap(i, j);
            }
        }

        self.playlist = files.into_iter().collect();
        Ok(())
    }

    fn scan_directory_recursive(
        &self,
        dir: &Path,
        files: &mut Vec<PathBuf>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let entries = fs::read_dir(dir)?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                self.scan_directory_recursive(&path, files)?;
            } else if self.is_audio_file(&path) {
                debug!("Found audio file: {:?}", path);
                files.push(path);
            }
        }

        Ok(())
    }

    fn is_audio_file(&self, path: &Path) -> bool {
        if let Some(extension) = path.extension() {
            if let Some(ext_str) = extension.to_str() {
                matches!(
                    ext_str.to_lowercase().as_str(),
                    "mp3" | "wav" | "flac" | "ogg" | "aac" | "m4a" | "opus" | "wma"
                )
            } else {
                false
            }
        } else {
            false
        }
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
        if self.scan_music_directory().is_ok() {
            self.current_index = 0;
            self.playlist_source = PlaylistSource::Library;
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
