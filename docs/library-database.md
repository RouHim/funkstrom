# Library Database Feature

## Overview

Transform the music library from in-memory scanning to a persistent SQLite-backed system that scans once on first run and tracks changes incrementally on subsequent startups.

## Motivation

**Current Behavior (Problem)**:
- Full filesystem scan on every startup
- Metadata extraction repeated every time
- Slow startup for large libraries
- Memory inefficient (holds all tracks in memory)

**New Behavior (Solution)**:
- One-time initial scan on first run
- Incremental scans on subsequent startups (only check changed files)
- Fast startup (1-10 seconds vs minutes for large libraries)
- Database stores extracted metadata persistently
- Memory efficient (only current playlist in memory)

## Architecture

### Components

```
┌─────────────────┐
│    main.rs      │
│  - Initialize   │
│  - DB setup     │
│  - Run scans    │
└────────┬────────┘
         │
         ├──────────────┬──────────────┐
         ▼              ▼              ▼
┌──────────────┐ ┌─────────────┐ ┌──────────────┐
│ library_db.rs│ │library_     │ │audio_reader  │
│              │ │scanner.rs   │ │.rs           │
│ - Connection │ │             │ │              │
│   pooling    │ │ - Full scan │ │ - Load from  │
│ - CRUD ops   │ │ - Incr scan │ │   DB         │
│ - Queries    │ │ - Metadata  │ │ - No scan    │
└──────────────┘ └─────────────┘ └──────────────┘
         │              │
         ▼              ▼
┌────────────────────────────┐
│  ./data/database.db        │
│  SQLite Database           │
│  - tracks table            │
│  - library_metadata table  │
└────────────────────────────┘
```

### File Structure

```
./
├── data/                        [NEW]
│   └── database.db             [NEW] - SQLite database
├── src/
│   ├── library_db.rs           [NEW] - Database operations
│   ├── library_scanner.rs      [NEW] - Filesystem scanning
│   ├── audio_reader.rs         [MODIFIED] - Use DB, no scan
│   └── main.rs                 [MODIFIED] - Init DB, run scans
├── docs/
│   └── library-database.md     [NEW] - This document
└── config.toml                 [NO CHANGES]
```

## Database Schema

### tracks table

```sql
CREATE TABLE tracks (
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
);

CREATE INDEX idx_tracks_file_path ON tracks(file_path);
CREATE INDEX idx_tracks_artist ON tracks(artist);
CREATE INDEX idx_tracks_album ON tracks(album);
CREATE INDEX idx_tracks_last_modified ON tracks(last_modified);
```

**Fields**:
- `id` - Primary key
- `file_path` - Absolute path to audio file (unique)
- `title` - Extracted from ID3/metadata tags
- `artist` - Extracted from ID3/metadata tags
- `album` - Extracted from ID3/metadata tags
- `duration_seconds` - Track duration (optional, not currently extracted)
- `file_size` - File size in bytes
- `last_modified` - Unix timestamp of file modification time
- `file_extension` - File extension (mp3, flac, etc.)
- `created_at` - Unix timestamp when record was created
- `updated_at` - Unix timestamp when record was last updated

### library_metadata table

```sql
CREATE TABLE library_metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
```

**Metadata Keys**:
- `last_full_scan` - Timestamp of last complete library scan
- `last_incremental_scan` - Timestamp of last incremental scan
- `library_version` - Schema version (for future migrations)

## Implementation Details

### Dependencies

Add to `Cargo.toml`:
```toml
rusqlite = { version = "0.37", features = ["bundled"] }
r2d2 = "0.8"
r2d2_sqlite = "0.31"
```

### library_db.rs - Database Layer

**Key Struct**:
```rust
pub struct LibraryDatabase {
    pool: r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>,
}

#[derive(Debug, Clone)]
pub struct TrackRecord {
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
```

**Core Methods**:
```rust
impl LibraryDatabase {
    pub fn new(db_path: &str) -> Result<Self, Box<dyn std::error::Error>>;
    pub fn initialize_schema(&self) -> Result<(), Box<dyn std::error::Error>>;
    
    // Track operations
    pub fn insert_track(&self, track: &TrackRecord) -> Result<i64, Box<dyn std::error::Error>>;
    pub fn update_track(&self, track: &TrackRecord) -> Result<(), Box<dyn std::error::Error>>;
    pub fn delete_track(&self, file_path: &str) -> Result<(), Box<dyn std::error::Error>>;
    pub fn get_track_by_path(&self, file_path: &str) -> Result<Option<TrackRecord>, Box<dyn std::error::Error>>;
    pub fn get_all_tracks(&self) -> Result<Vec<TrackRecord>, Box<dyn std::error::Error>>;
    pub fn track_count(&self) -> Result<usize, Box<dyn std::error::Error>>;
    
    // Metadata operations
    pub fn get_metadata(&self, key: &str) -> Result<Option<String>, Box<dyn std::error::Error>>;
    pub fn set_metadata(&self, key: &str, value: &str) -> Result<(), Box<dyn std::error::Error>>;
}
```

**Connection Pooling**:
- Uses `r2d2` for connection pooling
- Default pool size: 5 connections
- Handles concurrent access from multiple threads
- Auto-reconnects on connection failure

### library_scanner.rs - Filesystem Scanner

**Key Struct**:
```rust
pub struct LibraryScanner {
    music_directory: PathBuf,
    db: LibraryDatabase,
}

#[derive(Debug)]
pub struct ScanResult {
    pub added: usize,
    pub updated: usize,
    pub deleted: usize,
    pub unchanged: usize,
    pub errors: Vec<String>,
}
```

**Core Methods**:
```rust
impl LibraryScanner {
    pub fn new(music_directory: PathBuf, db: LibraryDatabase) -> Self;
    
    // Full scan: scan entire library from scratch
    pub fn full_scan(&self) -> Result<ScanResult, Box<dyn std::error::Error>>;
    
    // Incremental scan: detect changes since last scan
    pub fn incremental_scan(&self) -> Result<ScanResult, Box<dyn std::error::Error>>;
}
```

**Scan Logic**:

**Full Scan**:
1. Recursively scan music directory
2. For each audio file:
   - Extract metadata using `audiotags` crate
   - Get file size and modification time
   - Create `TrackRecord`
3. Insert all tracks into database
4. Update `last_full_scan` metadata

**Incremental Scan**:
1. Load all tracks from database into `HashMap<file_path, (last_modified, id)>`
2. Recursively scan music directory
3. For each audio file found:
   - If not in database → **ADD** (extract metadata, insert)
   - If in database but `last_modified` differs → **UPDATE** (re-extract metadata, update)
   - If in database and `last_modified` same → **UNCHANGED** (skip)
4. For tracks in database but not found on filesystem → **DELETE**
5. Update `last_incremental_scan` metadata

**Supported Audio Formats**:
- MP3 (`.mp3`)
- FLAC (`.flac`)
- OGG (`.ogg`)
- WAV (`.wav`)
- AAC (`.aac`)
- M4A (`.m4a`)
- Opus (`.opus`)
- WMA (`.wma`)

### audio_reader.rs - Modified Behavior

**Changes**:

**Before**:
```rust
impl AudioReader {
    pub fn new(
        music_directory: PathBuf,
        shuffle: bool,
        repeat: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        // Scans filesystem on every construction
        let mut reader = Self { ... };
        reader.scan_music_directory()?;  // SLOW
        Ok(reader)
    }
    
    fn scan_music_directory(&mut self) -> Result<...> { ... }
    fn scan_directory_recursive(&self, ...) -> Result<...> { ... }
    fn is_audio_file(&self, ...) -> bool { ... }
}
```

**After**:
```rust
impl AudioReader {
    pub fn new(
        music_directory: PathBuf,
        shuffle: bool,
        repeat: bool,
        db: LibraryDatabase,  // NEW parameter
    ) -> Result<Self, Box<dyn std::error::Error>> {
        // Loads from database (fast)
        let tracks = db.get_all_tracks()?;
        
        if tracks.is_empty() {
            return Err("No tracks found in library database".into());
        }
        
        let mut playlist: VecDeque<PathBuf> = tracks
            .into_iter()
            .map(|t| PathBuf::from(t.file_path))
            .collect();
            
        if shuffle {
            // Apply shuffle logic
        }
        
        Ok(Self { ... })
    }
    
    // DELETE: scan_music_directory()
    // DELETE: scan_directory_recursive()
    // DELETE: is_audio_file()
}
```

**Breaking Change**: `AudioReader::new()` signature now requires `LibraryDatabase` parameter.

### main.rs - Initialization Flow

**Startup Sequence**:

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    
    let config_path = get_config_path();
    let config = Config::from_file(&config_path)?;
    
    // 1. Create data directory
    fs::create_dir_all("./data")?;
    
    // 2. Initialize database
    let db = LibraryDatabase::new("./data/database.db")?;
    db.initialize_schema()?;
    
    // 3. Initialize scanner
    let scanner = LibraryScanner::new(
        PathBuf::from(&config.library.music_directory),
        db.clone()
    );
    
    // 4. Scan strategy
    let track_count = db.track_count()?;
    if track_count == 0 {
        // First run: full scan
        log::info!("Empty library, performing initial full scan...");
        let result = scanner.full_scan()?;
        log::info!("Initial scan complete: {} tracks added", result.added);
    } else {
        // Subsequent runs: incremental scan
        log::info!("Performing incremental library scan...");
        let result = scanner.incremental_scan()?;
        if result.added > 0 || result.updated > 0 || result.deleted > 0 {
            log::info!("Library changes: +{} ~{} -{} tracks", 
                result.added, result.updated, result.deleted);
        } else {
            log::info!("No library changes detected");
        }
    }
    
    // 5. Initialize audio reader with database
    let audio_reader = AudioReader::new(
        PathBuf::from(&config.library.music_directory),
        config.library.shuffle,
        config.library.repeat,
        db,  // Pass database
    )?;
    
    // 6. Continue with rest of initialization...
}
```

## Configuration

**No configuration changes required!**

The existing `config.toml` works as-is:
```toml
[library]
music_directory = "/path/to/music"
shuffle = true
repeat = true
```

**Hardcoded values**:
- Database path: `./data/database.db`
- Always perform incremental scan on startup
- No user configuration needed

## Startup Behavior

### First Run (Empty Database)

```
INFO: Created data directory: ./data
INFO: Database initialized at: ./data/database.db
INFO: Empty library, performing initial full scan...
INFO: Scanning music directory: "/home/user/music"
INFO: Found 150 audio files
INFO: Initial scan complete: 150 tracks added
INFO: Starting iRadio server on 127.0.0.1:8284
```

**Duration**: Depends on library size (e.g., 150 tracks ~ 10-20 seconds)

### Subsequent Runs (No Changes)

```
INFO: Database initialized at: ./data/database.db
INFO: Performing incremental library scan...
INFO: No library changes detected
INFO: Starting iRadio server on 127.0.0.1:8284
```

**Duration**: Fast (~1-5 seconds)

### Subsequent Runs (Files Changed)

```
INFO: Database initialized at: ./data/database.db
INFO: Performing incremental library scan...
INFO: Library changes: +5 ~2 -1 tracks
INFO: Starting iRadio server on 127.0.0.1:8284
```

**Duration**: Depends on number of changes (~1-10 seconds)

### Subsequent Runs (Large Library)

For 10,000+ tracks:
```
INFO: Database initialized at: ./data/database.db
INFO: Performing incremental library scan...
INFO: No library changes detected
INFO: Starting iRadio server on 127.0.0.1:8284
```

**Duration**: Still fast (~5-10 seconds) because only checks file modification times

## Performance Expectations

| Library Size | Initial Full Scan | Incremental Scan | Memory Usage |
|-------------|------------------|-----------------|-------------|
| 100 tracks | 5-10s | 1-2s | ~5 MB |
| 1,000 tracks | 10-20s | 1-2s | ~5-10 MB |
| 10,000 tracks | 2-5 min | 5-10s | ~10-20 MB |
| 100,000 tracks | 20-50 min | 30-60s | ~50-100 MB |

**Note**: Times assume modern SSD. HDD may be slower.

## Testing Strategy

### Unit Tests

**library_db.rs tests**:
- Schema creation
- Insert track
- Update track
- Delete track
- Get track by path
- Get all tracks
- Track count
- Metadata operations
- Connection pool behavior

**library_scanner.rs tests**:
- Full scan with test fixtures
- Incremental scan: detect new files
- Incremental scan: detect modified files
- Incremental scan: detect deleted files
- Incremental scan: skip unchanged files
- Audio file extension detection
- Error handling for inaccessible files
- Metadata extraction

### Integration Tests

- Full workflow: scan → DB → load → playback
- Add file → rescan → verify added
- Modify file → rescan → verify updated
- Delete file → rescan → verify removed

### E2E Tests

Run existing `./e2e/test.sh` to ensure:
- Streaming still works
- Metadata updates correctly
- No API regressions

## Migration Guide

**For developers**:

1. Add dependencies to `Cargo.toml`
2. Create `src/library_db.rs`
3. Create `src/library_scanner.rs`
4. Modify `src/audio_reader.rs`:
   - Add `db: LibraryDatabase` parameter to `new()`
   - Remove `scan_music_directory()`
   - Remove `scan_directory_recursive()`
   - Remove `is_audio_file()`
5. Modify `src/main.rs`:
   - Initialize database
   - Run scans
   - Pass database to `AudioReader::new()`
6. Add `mod library_db;` and `mod library_scanner;` to `main.rs`
7. Run tests
8. Run E2E tests

**For users**:

No changes needed! The feature is transparent:
- First run will take longer (initial scan)
- Subsequent runs are faster (incremental scan)
- Config file unchanged
- API unchanged

## Benefits

✅ **Fast startup**: No rescanning on every restart  
✅ **Scalable**: Supports libraries with 100k+ tracks  
✅ **Memory efficient**: Database stores metadata, not held in memory  
✅ **Incremental updates**: Only check changed files  
✅ **Persistent metadata**: Extracted tags saved permanently  
✅ **No config changes**: Zero user configuration needed  
✅ **No API changes**: Zero impact on external interfaces  
✅ **Future-ready**: Foundation for advanced features (search, stats, play counts)

## Future Enhancements (Not in Scope)

Possible future improvements:
- Manual rescan CLI command (`--rescan-library`)
- API endpoint for library statistics (`GET /library/stats`)
- Search by artist/album/title
- Track play counts
- Recently added tracks view
- Automatic background rescans (periodic)
- Watch filesystem for real-time updates (`inotify`/`fsnotify`)

## Troubleshooting

**Problem**: Database corrupted

**Solution**: Delete `./data/database.db` and restart. Full scan will run automatically.

---

**Problem**: Tracks not appearing after adding to music directory

**Solution**: Restart application. Incremental scan runs on startup.

---

**Problem**: Old deleted files still in playlist

**Solution**: Restart application. Incremental scan will detect deletions.

---

**Problem**: Slow incremental scan

**Solution**: Check disk I/O. Incremental scan must stat all files to check modification times.

## Implementation Checklist

- [ ] Add dependencies to `Cargo.toml`
- [ ] Create `src/library_db.rs`
- [ ] Create `src/library_scanner.rs`
- [ ] Modify `src/audio_reader.rs`
- [ ] Modify `src/main.rs`
- [ ] Write unit tests for `library_db.rs`
- [ ] Write unit tests for `library_scanner.rs`
- [ ] Run E2E tests
- [ ] Update `.gitignore` to exclude `/data/`
- [ ] Test with large library (1000+ tracks)

## References

- Database: SQLite via `rusqlite` crate
- Connection pooling: `r2d2` + `r2d2_sqlite`
- Metadata extraction: `audiotags` crate (already used)
- Schema design: Based on common music library patterns
