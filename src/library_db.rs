use log::info;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, Result as SqliteResult};
use std::error::Error;

#[derive(Debug, Clone)]
pub struct TrackRecord {
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

        for track in tracks {
            tx.execute(
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
        }

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

        for track in tracks {
            tx.execute(
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
        }

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

        for file_path in file_paths {
            tx.execute(
                "DELETE FROM tracks WHERE file_path = ?1",
                params![file_path],
            )?;
        }

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

    pub fn get_track_keys(&self) -> Result<Vec<(i64, String, i64)>, Box<dyn Error>> {
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
