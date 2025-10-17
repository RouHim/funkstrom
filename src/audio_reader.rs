use crate::audio_metadata::TrackMetadata;
use crossbeam_channel::{unbounded, Receiver};
use log::{debug, error, info};
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct AudioReader {
    music_directory: PathBuf,
    shuffle: bool,
    repeat: bool,
    playlist: VecDeque<PathBuf>,
    current_index: usize,
    current_metadata: Arc<Mutex<TrackMetadata>>,
}

impl AudioReader {
    pub fn new(
        music_directory: PathBuf,
        shuffle: bool,
        repeat: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut reader = Self {
            music_directory,
            shuffle,
            repeat,
            playlist: VecDeque::new(),
            current_index: 0,
            current_metadata: Arc::new(Mutex::new(TrackMetadata::default())),
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

        if self.shuffle {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};

            // Simple shuffle using current time as seed
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
            if self.repeat {
                self.current_index = 0;
                if self.shuffle {
                    // Re-shuffle for next iteration
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
                // End of playlist, no repeat
                return None;
            }
        }

        track
    }

    pub fn start_playlist_service(mut self) -> Receiver<PathBuf> {
        let (track_tx, track_rx) = unbounded::<PathBuf>();

        tokio::spawn(async move {
            loop {
                // Send next track
                if let Some(track) = self.next_track() {
                    info!("Next track: {:?}", track);
                    if track_tx.send(track).is_err() {
                        error!("Failed to send track to channel");
                        break;
                    }
                } else {
                    info!("End of playlist reached");
                    if !self.repeat {
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
