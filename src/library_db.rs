use log::info;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, OptionalExtension, Result as SqliteResult};
use std::error::Error;

type TrackKey = (i64, String, i64);

#[derive(Debug, Clone)]
pub struct TrackRecord {
    #[allow(dead_code)] // Field populated from database, used for internal tracking
    pub id: Option<i64>,
    pub file_path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_seconds: Option<i64>,
    pub file_size: i64,
    pub last_modified: i64,
    pub file_extension: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone)]
pub struct LibraryDatabase {
    pool: Pool<SqliteConnectionManager>,
}

impl LibraryDatabase {
    pub fn new(db_path: &str) -> Result<Self, Box<dyn Error>> {
        let manager = SqliteConnectionManager::file(db_path);
        let pool = Pool::builder().max_size(5).build(manager)?;

        info!("Database initialized at: {}", db_path);

        Ok(Self { pool })
    }

    pub fn initialize_schema(&self) -> Result<(), Box<dyn Error>> {
        let mut conn = self.pool.get()?;

        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;

        let tx = conn.transaction()?;

        tx.execute(
            "CREATE TABLE IF NOT EXISTS tracks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_path TEXT NOT NULL UNIQUE,
                title TEXT NOT NULL,
                artist TEXT NOT NULL,
                album TEXT NOT NULL,
                duration_seconds INTEGER,
                file_size INTEGER NOT NULL,
                last_modified INTEGER NOT NULL,
                file_extension TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;

        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_tracks_file_path ON tracks(file_path)",
            [],
        )?;

        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_tracks_artist ON tracks(artist)",
            [],
        )?;

        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_tracks_album ON tracks(album)",
            [],
        )?;

        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_tracks_last_modified ON tracks(last_modified)",
            [],
        )?;

        tx.execute(
            "CREATE TABLE IF NOT EXISTS library_metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;

        tx.commit()?;

        Ok(())
    }

    pub fn insert_track(&self, track: &TrackRecord) -> Result<i64, Box<dyn Error>> {
        let conn = self.pool.get()?;

        conn.execute(
            "INSERT INTO tracks (file_path, title, artist, album, duration_seconds, 
                file_size, last_modified, file_extension, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                track.file_path,
                track.title,
                track.artist,
                track.album,
                track.duration_seconds,
                track.file_size,
                track.last_modified,
                track.file_extension,
                track.created_at,
                track.updated_at,
            ],
        )?;

        Ok(conn.last_insert_rowid())
    }

    pub fn insert_tracks_batch(&self, tracks: &[TrackRecord]) -> Result<(), Box<dyn Error>> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;

        let mut stmt = tx.prepare(
            "INSERT INTO tracks (file_path, title, artist, album, duration_seconds, 
                file_size, last_modified, file_extension, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?;

        for track in tracks {
            stmt.execute(params![
                track.file_path,
                track.title,
                track.artist,
                track.album,
                track.duration_seconds,
                track.file_size,
                track.last_modified,
                track.file_extension,
                track.created_at,
                track.updated_at,
            ])?;
        }

        drop(stmt);
        tx.commit()?;

        Ok(())
    }

    pub fn update_track(&self, track: &TrackRecord) -> Result<(), Box<dyn Error>> {
        let conn = self.pool.get()?;

        conn.execute(
            "UPDATE tracks SET title = ?1, artist = ?2, album = ?3, duration_seconds = ?4,
                file_size = ?5, last_modified = ?6, file_extension = ?7, updated_at = ?8
             WHERE file_path = ?9",
            params![
                track.title,
                track.artist,
                track.album,
                track.duration_seconds,
                track.file_size,
                track.last_modified,
                track.file_extension,
                track.updated_at,
                track.file_path,
            ],
        )?;

        Ok(())
    }

    pub fn update_tracks_batch(&self, tracks: &[TrackRecord]) -> Result<(), Box<dyn Error>> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;

        let mut stmt = tx.prepare(
            "UPDATE tracks SET title = ?1, artist = ?2, album = ?3, duration_seconds = ?4,
                file_size = ?5, last_modified = ?6, file_extension = ?7, updated_at = ?8
             WHERE file_path = ?9",
        )?;

        for track in tracks {
            stmt.execute(params![
                track.title,
                track.artist,
                track.album,
                track.duration_seconds,
                track.file_size,
                track.last_modified,
                track.file_extension,
                track.updated_at,
                track.file_path,
            ])?;
        }

        drop(stmt);
        tx.commit()?;

        Ok(())
    }

    pub fn delete_track(&self, file_path: &str) -> Result<(), Box<dyn Error>> {
        let conn = self.pool.get()?;
        conn.execute(
            "DELETE FROM tracks WHERE file_path = ?1",
            params![file_path],
        )?;
        Ok(())
    }

    pub fn delete_tracks_batch(&self, file_paths: &[String]) -> Result<(), Box<dyn Error>> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;

        let mut stmt = tx.prepare("DELETE FROM tracks WHERE file_path = ?1")?;

        for file_path in file_paths {
            stmt.execute(params![file_path])?;
        }

        drop(stmt);
        tx.commit()?;

        Ok(())
    }

    pub fn get_all_tracks(&self) -> Result<Vec<TrackRecord>, Box<dyn Error>> {
        let conn = self.pool.get()?;

        let mut stmt = conn.prepare(
            "SELECT id, file_path, title, artist, album, duration_seconds,
                file_size, last_modified, file_extension, created_at, updated_at
             FROM tracks",
        )?;

        let tracks = stmt
            .query_map([], |row| {
                Ok(TrackRecord {
                    id: row.get(0)?,
                    file_path: row.get(1)?,
                    title: row.get(2)?,
                    artist: row.get(3)?,
                    album: row.get(4)?,
                    duration_seconds: row.get(5)?,
                    file_size: row.get(6)?,
                    last_modified: row.get(7)?,
                    file_extension: row.get(8)?,
                    created_at: row.get(9)?,
                    updated_at: row.get(10)?,
                })
            })?
            .collect::<SqliteResult<Vec<_>>>()?;

        Ok(tracks)
    }

    pub fn get_track_keys(&self) -> Result<Vec<TrackKey>, Box<dyn Error>> {
        let conn = self.pool.get()?;

        let mut stmt = conn.prepare("SELECT id, file_path, last_modified FROM tracks")?;

        let keys = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<SqliteResult<Vec<_>>>()?;

        Ok(keys)
    }

    pub fn track_count(&self) -> Result<usize, Box<dyn Error>> {
        let conn = self.pool.get()?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    pub fn get_metadata(&self, key: &str) -> Result<Option<String>, Box<dyn Error>> {
        let conn = self.pool.get()?;
        let result = conn
            .query_row(
                "SELECT value FROM library_metadata WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(result)
    }

    pub fn set_metadata(&self, key: &str, value: &str) -> Result<(), Box<dyn Error>> {
        let conn = self.pool.get()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;

        conn.execute(
            "INSERT OR REPLACE INTO library_metadata (key, value, updated_at)
             VALUES (?1, ?2, ?3)",
            params![key, value, now],
        )?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (LibraryDatabase, NamedTempFile) {
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_str().unwrap();
        let db = LibraryDatabase::new(db_path).unwrap();
        db.initialize_schema().unwrap();
        (db, temp_file)
    }

    fn create_test_track(file_path: &str) -> TrackRecord {
        TrackRecord {
            id: None,
            file_path: file_path.to_string(),
            title: "Test Song".to_string(),
            artist: "Test Artist".to_string(),
            album: "Test Album".to_string(),
            duration_seconds: Some(180),
            file_size: 3000000,
            last_modified: 1234567890,
            file_extension: "mp3".to_string(),
            created_at: 1234567890,
            updated_at: 1234567890,
        }
    }

    #[test]
    fn given_new_database_when_schema_initialized_then_tables_created() {
        let (db, _temp) = create_test_db();

        let count = db.track_count().unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn given_track_when_inserted_then_returns_id_and_can_be_retrieved() {
        let (db, _temp) = create_test_db();
        let track = create_test_track("/music/song1.mp3");

        let id = db.insert_track(&track).unwrap();

        assert!(id > 0);
        let tracks = db.get_all_tracks().unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].file_path, "/music/song1.mp3");
        assert_eq!(tracks[0].title, "Test Song");
        assert_eq!(tracks[0].id, Some(id));
    }

    #[test]
    fn given_multiple_tracks_when_inserted_in_batch_then_all_tracks_saved() {
        let (db, _temp) = create_test_db();
        let tracks = vec![
            create_test_track("/music/song1.mp3"),
            create_test_track("/music/song2.mp3"),
            create_test_track("/music/song3.mp3"),
        ];

        db.insert_tracks_batch(&tracks).unwrap();

        let saved_tracks = db.get_all_tracks().unwrap();
        assert_eq!(saved_tracks.len(), 3);
    }

    #[test]
    fn given_existing_track_when_updated_then_changes_persisted() {
        let (db, _temp) = create_test_db();
        let mut track = create_test_track("/music/song1.mp3");
        db.insert_track(&track).unwrap();

        track.title = "Updated Title".to_string();
        track.artist = "Updated Artist".to_string();
        track.updated_at = 9999999999;
        db.update_track(&track).unwrap();

        let tracks = db.get_all_tracks().unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Updated Title");
        assert_eq!(tracks[0].artist, "Updated Artist");
        assert_eq!(tracks[0].updated_at, 9999999999);
    }

    #[test]
    fn given_multiple_tracks_when_batch_updated_then_all_changes_persisted() {
        let (db, _temp) = create_test_db();
        let tracks = vec![
            create_test_track("/music/song1.mp3"),
            create_test_track("/music/song2.mp3"),
        ];
        db.insert_tracks_batch(&tracks).unwrap();

        let mut updated_tracks = tracks.clone();
        updated_tracks[0].title = "Updated 1".to_string();
        updated_tracks[1].title = "Updated 2".to_string();
        db.update_tracks_batch(&updated_tracks).unwrap();

        let saved_tracks = db.get_all_tracks().unwrap();
        assert_eq!(saved_tracks[0].title, "Updated 1");
        assert_eq!(saved_tracks[1].title, "Updated 2");
    }

    #[test]
    fn given_existing_track_when_deleted_then_removed_from_database() {
        let (db, _temp) = create_test_db();
        let track = create_test_track("/music/song1.mp3");
        db.insert_track(&track).unwrap();

        db.delete_track(&track.file_path).unwrap();

        let tracks = db.get_all_tracks().unwrap();
        assert_eq!(tracks.len(), 0);
    }

    #[test]
    fn given_multiple_tracks_when_batch_deleted_then_all_removed() {
        let (db, _temp) = create_test_db();
        let tracks = vec![
            create_test_track("/music/song1.mp3"),
            create_test_track("/music/song2.mp3"),
            create_test_track("/music/song3.mp3"),
        ];
        db.insert_tracks_batch(&tracks).unwrap();

        let paths_to_delete = vec![
            "/music/song1.mp3".to_string(),
            "/music/song3.mp3".to_string(),
        ];
        db.delete_tracks_batch(&paths_to_delete).unwrap();

        let remaining = db.get_all_tracks().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].file_path, "/music/song2.mp3");
    }

    #[test]
    fn given_tracks_in_database_when_get_track_keys_called_then_returns_lightweight_data() {
        let (db, _temp) = create_test_db();
        let tracks = vec![
            create_test_track("/music/song1.mp3"),
            create_test_track("/music/song2.mp3"),
        ];
        db.insert_tracks_batch(&tracks).unwrap();

        let keys = db.get_track_keys().unwrap();

        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].1, "/music/song1.mp3");
        assert_eq!(keys[0].2, 1234567890);
        assert_eq!(keys[1].1, "/music/song2.mp3");
        assert_eq!(keys[1].2, 1234567890);
    }

    #[test]
    fn given_empty_database_when_track_count_called_then_returns_zero() {
        let (db, _temp) = create_test_db();

        let count = db.track_count().unwrap();

        assert_eq!(count, 0);
    }

    #[test]
    fn given_tracks_in_database_when_track_count_called_then_returns_correct_count() {
        let (db, _temp) = create_test_db();
        let tracks = vec![
            create_test_track("/music/song1.mp3"),
            create_test_track("/music/song2.mp3"),
            create_test_track("/music/song3.mp3"),
        ];
        db.insert_tracks_batch(&tracks).unwrap();

        let count = db.track_count().unwrap();

        assert_eq!(count, 3);
    }

    #[test]
    fn given_metadata_when_set_and_retrieved_then_values_match() {
        let (db, _temp) = create_test_db();

        db.set_metadata("last_scan", "2024-01-01").unwrap();

        let value = db.get_metadata("last_scan").unwrap();
        assert_eq!(value, Some("2024-01-01".to_string()));
    }

    #[test]
    fn given_nonexistent_metadata_key_when_retrieved_then_returns_none() {
        let (db, _temp) = create_test_db();

        let value = db.get_metadata("nonexistent").unwrap();

        assert_eq!(value, None);
    }

    #[test]
    fn given_existing_metadata_when_updated_then_new_value_persisted() {
        let (db, _temp) = create_test_db();
        db.set_metadata("key", "old_value").unwrap();

        db.set_metadata("key", "new_value").unwrap();

        let value = db.get_metadata("key").unwrap();
        assert_eq!(value, Some("new_value".to_string()));
    }

    #[test]
    fn given_duplicate_file_path_when_inserted_then_returns_error() {
        let (db, _temp) = create_test_db();
        let track = create_test_track("/music/song1.mp3");
        db.insert_track(&track).unwrap();

        let result = db.insert_track(&track);

        assert!(result.is_err());
    }
}
