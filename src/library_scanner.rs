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

        for file_path in files {
            match self.process_file(&file_path) {
                Ok(track) => match self.db.insert_track(&track) {
                    Ok(_) => {
                        debug!("Added track: {}", track.file_path);
                        result.added += 1;
                    }
                    Err(e) => {
                        warn!("Failed to insert track {}: {}", track.file_path, e);
                        result.errors.push(format!("{}: {}", track.file_path, e));
                    }
                },
                Err(e) => {
                    warn!("Failed to process file {:?}: {}", file_path, e);
                    result.errors.push(format!("{:?}: {}", file_path, e));
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

        let existing_tracks = self.db.get_all_tracks()?;
        let mut existing_map: HashMap<String, (i64, i64)> = existing_tracks
            .into_iter()
            .filter_map(|t| t.id.map(|id| (t.file_path.clone(), (t.last_modified, id))))
            .collect();

        let mut files = Vec::new();
        self.scan_directory_recursive(&self.music_directory, &mut files)?;

        for file_path in files {
            let file_path_str = file_path.to_string_lossy().to_string();

            match self.get_file_mtime(&file_path) {
                Ok(current_mtime) => {
                    if let Some((db_mtime, _)) = existing_map.remove(&file_path_str) {
                        if current_mtime != db_mtime {
                            match self.process_file(&file_path) {
                                Ok(track) => match self.db.update_track(&track) {
                                    Ok(_) => {
                                        debug!("Updated track: {}", track.file_path);
                                        result.updated += 1;
                                    }
                                    Err(e) => {
                                        warn!("Failed to update track {}: {}", track.file_path, e);
                                        result.errors.push(format!("{}: {}", track.file_path, e));
                                    }
                                },
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
                            Ok(track) => match self.db.insert_track(&track) {
                                Ok(_) => {
                                    debug!("Added new track: {}", track.file_path);
                                    result.added += 1;
                                }
                                Err(e) => {
                                    warn!("Failed to insert track {}: {}", track.file_path, e);
                                    result.errors.push(format!("{}: {}", track.file_path, e));
                                }
                            },
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

        for (deleted_path, _) in existing_map {
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
