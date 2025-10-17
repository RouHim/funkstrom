use crate::library_db::{LibraryDatabase, TrackRecord};
use audiotags::Tag;
use log::{debug, info, warn};
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub struct ScanResult {
    pub added: usize,
    pub updated: usize,
    pub deleted: usize,
    pub unchanged: usize,
    pub errors: Vec<String>,
}

pub struct LibraryScanner {
    music_directory: PathBuf,
    db: LibraryDatabase,
}

impl LibraryScanner {
    pub fn new(music_directory: PathBuf, db: LibraryDatabase) -> Self {
        Self {
            music_directory,
            db,
        }
    }

    pub fn full_scan(&self) -> Result<ScanResult, Box<dyn Error>> {
        info!("Starting full library scan in: {:?}", self.music_directory);

        let mut result = ScanResult {
            added: 0,
            updated: 0,
            deleted: 0,
            unchanged: 0,
            errors: Vec::new(),
        };

        let mut files = Vec::new();
        self.scan_directory_recursive(&self.music_directory, &mut files)?;

        info!("Found {} audio files", files.len());

        let mut tracks = Vec::new();

        for file_path in files {
            match self.process_file(&file_path) {
                Ok(track) => {
                    tracks.push(track);
                }
                Err(e) => {
                    warn!("Failed to process file {:?}: {}", file_path, e);
                    result.errors.push(format!("{:?}: {}", file_path, e));
                }
            }
        }

        match self.db.insert_tracks_batch(&tracks) {
            Ok(_) => {
                result.added = tracks.len();
                info!("Inserted {} tracks in batch", tracks.len());
            }
            Err(e) => {
                warn!(
                    "Batch insert failed: {}, falling back to individual inserts",
                    e
                );
                for track in tracks {
                    match self.db.insert_track(&track) {
                        Ok(_) => {
                            debug!("Added track: {}", track.file_path);
                            result.added += 1;
                        }
                        Err(e) => {
                            warn!("Failed to insert track {}: {}", track.file_path, e);
                            result.errors.push(format!("{}: {}", track.file_path, e));
                        }
                    }
                }
            }
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs()
            .to_string();
        self.db.set_metadata("last_full_scan", &now)?;

        info!(
            "Full scan complete: +{} added, {} errors",
            result.added,
            result.errors.len()
        );

        Ok(result)
    }

    pub fn incremental_scan(&self) -> Result<ScanResult, Box<dyn Error>> {
        info!("Starting incremental library scan");

        let mut result = ScanResult {
            added: 0,
            updated: 0,
            deleted: 0,
            unchanged: 0,
            errors: Vec::new(),
        };

        let track_keys = self.db.get_track_keys()?;
        let mut existing_map: HashMap<String, (i64, i64)> = track_keys
            .into_iter()
            .map(|(id, file_path, last_modified)| (file_path, (last_modified, id)))
            .collect();

        let mut files = Vec::new();
        self.scan_directory_recursive(&self.music_directory, &mut files)?;

        let mut tracks_to_add = Vec::new();
        let mut tracks_to_update = Vec::new();

        for file_path in files {
            let file_path_str = file_path.to_string_lossy().to_string();

            match self.get_file_mtime(&file_path) {
                Ok(current_mtime) => {
                    if let Some((db_mtime, _)) = existing_map.remove(&file_path_str) {
                        if current_mtime != db_mtime {
                            match self.process_file(&file_path) {
                                Ok(track) => {
                                    tracks_to_update.push(track);
                                }
                                Err(e) => {
                                    warn!("Failed to process file {:?}: {}", file_path, e);
                                    result.errors.push(format!("{:?}: {}", file_path, e));
                                }
                            }
                        } else {
                            result.unchanged += 1;
                        }
                    } else {
                        match self.process_file(&file_path) {
                            Ok(track) => {
                                tracks_to_add.push(track);
                            }
                            Err(e) => {
                                warn!("Failed to process file {:?}: {}", file_path, e);
                                result.errors.push(format!("{:?}: {}", file_path, e));
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to get mtime for {:?}: {}", file_path, e);
                    result.errors.push(format!("{:?}: {}", file_path, e));
                }
            }
        }

        if !tracks_to_add.is_empty() {
            match self.db.insert_tracks_batch(&tracks_to_add) {
                Ok(_) => {
                    result.added = tracks_to_add.len();
                    info!("Added {} tracks in batch", tracks_to_add.len());
                }
                Err(e) => {
                    warn!(
                        "Batch insert failed: {}, falling back to individual inserts",
                        e
                    );
                    for track in tracks_to_add {
                        match self.db.insert_track(&track) {
                            Ok(_) => {
                                debug!("Added new track: {}", track.file_path);
                                result.added += 1;
                            }
                            Err(e) => {
                                warn!("Failed to insert track {}: {}", track.file_path, e);
                                result.errors.push(format!("{}: {}", track.file_path, e));
                            }
                        }
                    }
                }
            }
        }

        if !tracks_to_update.is_empty() {
            match self.db.update_tracks_batch(&tracks_to_update) {
                Ok(_) => {
                    result.updated = tracks_to_update.len();
                    info!("Updated {} tracks in batch", tracks_to_update.len());
                }
                Err(e) => {
                    warn!(
                        "Batch update failed: {}, falling back to individual updates",
                        e
                    );
                    for track in tracks_to_update {
                        match self.db.update_track(&track) {
                            Ok(_) => {
                                debug!("Updated track: {}", track.file_path);
                                result.updated += 1;
                            }
                            Err(e) => {
                                warn!("Failed to update track {}: {}", track.file_path, e);
                                result.errors.push(format!("{}: {}", track.file_path, e));
                            }
                        }
                    }
                }
            }
        }

        let deleted_paths: Vec<String> = existing_map.into_keys().collect();

        if !deleted_paths.is_empty() {
            match self.db.delete_tracks_batch(&deleted_paths) {
                Ok(_) => {
                    result.deleted = deleted_paths.len();
                    info!("Deleted {} tracks in batch", deleted_paths.len());
                }
                Err(e) => {
                    warn!(
                        "Batch delete failed: {}, falling back to individual deletes",
                        e
                    );
                    for deleted_path in deleted_paths {
                        match self.db.delete_track(&deleted_path) {
                            Ok(_) => {
                                debug!("Deleted track: {}", deleted_path);
                                result.deleted += 1;
                            }
                            Err(e) => {
                                warn!("Failed to delete track {}: {}", deleted_path, e);
                                result.errors.push(format!("{}: {}", deleted_path, e));
                            }
                        }
                    }
                }
            }
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs()
            .to_string();
        self.db.set_metadata("last_incremental_scan", &now)?;

        if result.added > 0 || result.updated > 0 || result.deleted > 0 {
            info!(
                "Incremental scan complete: +{} added, ~{} updated, -{} deleted, {} unchanged, {} errors",
                result.added, result.updated, result.deleted, result.unchanged, result.errors.len()
            );
        } else {
            info!("No library changes detected");
        }

        Ok(result)
    }

    fn scan_directory_recursive(
        &self,
        dir: &Path,
        files: &mut Vec<PathBuf>,
    ) -> Result<(), Box<dyn Error>> {
        let entries = fs::read_dir(dir)?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                self.scan_directory_recursive(&path, files)?;
            } else if self.is_audio_file(&path) {
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

    fn process_file(&self, path: &PathBuf) -> Result<TrackRecord, Box<dyn Error>> {
        let file_path = path.to_string_lossy().to_string();
        let metadata = fs::metadata(path)?;
        let file_size = metadata.len() as i64;
        let last_modified = metadata.modified()?.duration_since(UNIX_EPOCH)?.as_secs() as i64;

        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        let (title, artist, album) = match Tag::new().read_from_path(path) {
            Ok(tag) => {
                let title = tag.title().map(|s| s.to_string()).unwrap_or_else(|| {
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Unknown")
                        .to_string()
                });
                let artist = tag
                    .artist()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "Unknown Artist".to_string());
                let album = tag
                    .album()
                    .map(|a| a.title.to_string())
                    .unwrap_or_else(|| "Unknown Album".to_string());
                (title, artist, album)
            }
            Err(e) => {
                debug!("Failed to read tags from {:?}: {}", path, e);
                let title = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Unknown")
                    .to_string();
                (
                    title,
                    "Unknown Artist".to_string(),
                    "Unknown Album".to_string(),
                )
            }
        };

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;

        Ok(TrackRecord {
            id: None,
            file_path,
            title,
            artist,
            album,
            duration_seconds: None,
            file_size,
            last_modified,
            file_extension: extension,
            created_at: now,
            updated_at: now,
        })
    }

    fn get_file_mtime(&self, path: &Path) -> Result<i64, Box<dyn Error>> {
        let metadata = fs::metadata(path)?;
        let mtime = metadata.modified()?.duration_since(UNIX_EPOCH)?.as_secs() as i64;
        Ok(mtime)
    }
}
