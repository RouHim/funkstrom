# Configuration Guide

This document provides comprehensive documentation for all configuration options available in Funkstrom's `config.toml`
file.

## Table of Contents

- [Quick Start](#quick-start)
- [Overview](#overview)
- [Server Configuration](#server-configuration)
- [Library Configuration](#library-configuration)
- [Station Configuration](#station-configuration)
- [Stream Configuration](#stream-configuration)
- [Schedule Configuration](#schedule-configuration)
- [M3U Playlist Format](#m3u-playlist-format)
- [HTTP API Reference](#http-api-reference)
- [Database](#database)
- [Frequently Asked Questions](#frequently-asked-questions)
- [Complete Examples](#complete-examples)

## Quick Start

### Minimal Working Configuration

Create `config.toml`:

```toml
[server]
port = 8284
bind_address = "127.0.0.1"

[library]
music_directory = "/path/to/your/music"
shuffle = true
repeat = true

[station]
station_name = "My Radio"
description = "My personal radio station"
genre = "Various"
url = "http://localhost:8284"

[stream.default]
bitrate = 128
format = "mp3"
sample_rate = 44100
channels = 2
enabled = true
```

### Start Server

```bash
RUST_LOG=info cargo run -- --config config.toml
```

### Listen

Open `http://localhost:8284/default` in a media player (VLC, mpv) or browser.

**Note:** First startup will scan your music directory - this may take a few minutes for large libraries.

## Overview

Funkstrom uses a TOML configuration file to define all aspects of the radio streaming server. The configuration is
divided into five main sections:

1. **Server** - Network and system settings
2. **Library** - Music collection settings
3. **Station** - Station metadata and information
4. **Stream** - Audio stream quality and format settings (supports multiple streams)
5. **Schedule** - Optional timed programming

## Server Configuration

The `[server]` section controls network binding and system paths.

### Options

| Option         | Type    | Required | Default    | Description                           |
|----------------|---------|----------|------------|---------------------------------------|
| `port`         | integer | Yes      | -          | Port number for HTTP server (1-65535) |
| `bind_address` | string  | Yes      | -          | IP address to bind to                 |
| `ffmpeg_path`  | string  | No       | `"ffmpeg"` | Path to ffmpeg binary                 |

### Details

#### `port`

The TCP port on which the HTTP server will listen for incoming connections. All streams and API endpoints will be
accessible on this port.

- **Valid values**: 1-65535 (recommended: 8000-9000 or above 1024 to avoid requiring root)
- **Example**: `8284`

#### `bind_address`

The network interface address to bind to. This controls which network interfaces the server listens on.

- **Values**:
    - `"127.0.0.1"` - Listen only on localhost (local access only)
    - `"0.0.0.0"` - Listen on all network interfaces (remote access allowed)
    - Specific IP - Listen only on a specific interface
- **Security note**: Use `127.0.0.1` for development or when using a reverse proxy

#### `ffmpeg_path`

Optional path to the ffmpeg binary. If not specified, the system will search for `ffmpeg` in your PATH environment
variable.

- **When to use**: Specify when ffmpeg is installed in a non-standard location
- **Examples**:
    - `"/usr/bin/ffmpeg"`
    - `"/opt/ffmpeg/bin/ffmpeg"`
    - `"/home/user/.local/bin/ffmpeg"`

### Example

```toml
[server]
port = 8284
bind_address = "127.0.0.1"
ffmpeg_path = "/usr/bin/ffmpeg"
```

## Library Configuration

The `[library]` section defines your music collection and playback behavior.

### Options

| Option            | Type    | Required | Default | Description               |
|-------------------|---------|----------|---------|---------------------------|
| `music_directory` | string  | Yes      | -       | Path to music files       |
| `shuffle`         | boolean | Yes      | -       | Shuffle playback order    |
| `repeat`          | boolean | Yes      | -       | Repeat when playlist ends |

### Details

#### `music_directory`

The file system path containing your music collection. The server will recursively scan this directory for supported
audio files.

- **Supported formats**: MP3, FLAC, OGG, M4A, WAV, and other ffmpeg-supported formats
- **Path requirements**: Must be an absolute path, must exist and be readable
- **Scanning**: The directory is scanned on startup and indexed in a SQLite database
- **Example**: `"/home/radio/music"` or `"/mnt/media/audio"`

#### `shuffle`

Controls whether tracks are played in random order or sequentially by filename.

- **Values**:
    - `true` - Randomize playback order (recommended for music variety)
    - `false` - Play tracks in alphabetical/directory order
- **Behavior**: Shuffle order is maintained in the database and consistent across restarts

#### `repeat`

Controls whether playback loops back to the beginning when all tracks have been played.

- **Values**:
    - `true` - Restart from beginning when reaching the end (recommended for continuous streaming)
    - `false` - Stop playback when all tracks are exhausted
- **Use case**: Set to `true` for 24/7 operation

### Example

```toml
[library]
music_directory = "/home/radio/music"
shuffle = true
repeat = true
```

## Station Configuration

The `[station]` section provides metadata about your radio station that is sent to listeners.

### Options

| Option         | Type   | Required | Default | Description          |
|----------------|--------|----------|---------|----------------------|
| `station_name` | string | Yes      | -       | Station display name |
| `description`  | string | Yes      | -       | Station description  |
| `genre`        | string | Yes      | -       | Music genre category |
| `url`          | string | Yes      | -       | Station website URL  |

### Details

#### `station_name`

The name of your radio station, displayed in audio players and the info page.

- **Visible in**: Icecast headers, player displays, status page
- **Example**: `"Deep Sea Radio"`, `"Jazz FM"`

#### `description`

A brief description of your station's content or programming.

- **Visible in**: Info page, station metadata
- **Example**: `"Underground electronic music 24/7"`

#### `genre`

The primary music genre of your station.

- **Purpose**: Helps listeners discover your station by genre
- **Examples**: `"Electronic"`, `"Jazz"`, `"Rock"`, `"Various"`, `"Techno"`

#### `url`

The web URL where listeners can find more information about your station.

- **Format**: Full URL including protocol
- **Examples**:
    - `"http://localhost:8284"` (for local testing)
    - `"https://radio.example.com"`

### Example

```toml
[station]
station_name = "Deep Sea Radio"
description = "Oceanic electronic beats"
genre = "Electronic"
url = "http://localhost:8284"
```

## Stream Configuration

The `[stream.<name>]` sections define individual audio streams with different quality settings. Funkstrom supports
multiple simultaneous streams, allowing listeners to choose their preferred quality/bandwidth trade-off.

### Stream Naming

Each stream is defined with a unique name that becomes part of its URL:

- Format: `[stream.<name>]`
- URL: `http://<bind_address>:<port>/<name>`
- **Name requirements**: Only alphanumeric characters, underscores, or hyphens
- **Examples**: `[stream.high]`, `[stream.mobile]`, `[stream.low_bandwidth]`

### Options

| Option        | Type    | Required | Default | Description              |
|---------------|---------|----------|---------|--------------------------|
| `bitrate`     | integer | Yes      | -       | Audio bitrate in kbps    |
| `format`      | string  | Yes      | -       | Audio codec format       |
| `sample_rate` | integer | Yes      | -       | Sample rate in Hz        |
| `channels`    | integer | Yes      | -       | Number of audio channels |
| `enabled`     | boolean | Yes      | -       | Enable/disable stream    |

### Details

#### `bitrate`

The audio bitrate in kilobits per second (kbps). Higher bitrate = better quality but more bandwidth.

- **Valid range**: 32-320 kbps
- **Recommended values**:
    - **32-64 kbps**: Mobile/low bandwidth (acceptable for speech)
    - **96-128 kbps**: Standard quality (good for most music)
    - **192-256 kbps**: High quality (excellent for all content)
    - **320 kbps**: Maximum quality (audiophile, high bandwidth)

#### `format`

The audio codec used for encoding the stream.

- **Supported formats**: `mp3`, `aac`, `opus`, `ogg`
- **Format characteristics**:
    - **mp3**: Universal compatibility, good quality at higher bitrates
    - **aac**: Better quality than MP3 at same bitrate, widely supported
    - **opus**: Best quality at low bitrates, modern browsers only
    - **ogg**: Open format, good quality, limited player support

#### `sample_rate`

The audio sample rate in Hertz. Higher sample rate = better frequency response.

- **Valid values**: `8000`, `11025`, `16000`, `22050`, `32000`, `44100`, `48000`
- **Common choices**:
    - **8000-16000 Hz**: Voice/podcast only
    - **22050 Hz**: Lower quality music, mobile streaming
    - **44100 Hz**: CD quality, standard for music
    - **48000 Hz**: Professional quality, modern standard

#### `channels`

Number of audio channels in the output.

- **Valid values**:
    - `1` - Mono (single channel, saves bandwidth)
    - `2` - Stereo (two channels, full spatial audio)
- **Recommendation**: Use stereo (2) for music, mono (1) for voice or to reduce bandwidth

#### `enabled`

Whether this stream is active and available to listeners.

- **Values**:
    - `true` - Stream is active and accessible
    - `false` - Stream is disabled (config preserved but not running)
- **Requirement**: At least one stream must be enabled

### Validation Rules

The server validates stream configuration on startup:

1. At least one stream must be defined
2. At least one stream must be enabled
3. Stream names must be non-empty and contain only alphanumeric, underscore, or hyphen characters
4. All stream parameters must be within valid ranges
5. Format must be one of the supported codecs

### Examples

#### High Quality Stream

```toml
[stream.high]
bitrate = 320
format = "mp3"
sample_rate = 48000
channels = 2
enabled = true
```

#### Standard Quality Stream

```toml
[stream.standard]
bitrate = 128
format = "mp3"
sample_rate = 44100
channels = 2
enabled = true
```

#### Mobile-Optimized Stream

```toml
[stream.mobile]
bitrate = 48
format = "aac"
sample_rate = 22050
channels = 1
enabled = true
```

#### Modern Opus Stream

```toml
[stream.opus]
bitrate = 96
format = "opus"
sample_rate = 48000
channels = 2
enabled = false  # Disabled by default
```

## Schedule Configuration

The optional `[schedule]` section enables time-based programming, allowing you to schedule specific playlists or content
to play at certain times.

### Overview

The schedule system uses cron expressions to define when programs should run. When a scheduled program is active, it
temporarily overrides the main library playback. Programs can be either:

- **Playlist programs** - Play local M3U playlist files
- **Liveset programs** - Stream electronic music livesets from hearthis.at API

### Structure

```toml
[schedule]
programs = [...]  # Array of program configurations

[[schedule.programs]]
# First program definition

[[schedule.programs]]
# Second program definition
```

### Program Options

| Option     | Type    | Required    | Default      | Description                                    |
|------------|---------|-------------|--------------|------------------------------------------------|
| `name`     | string  | Yes         | -            | Program display name                           |
| `active`   | boolean | Yes         | -            | Enable/disable program                         |
| `cron`     | string  | Yes         | -            | Cron schedule expression                       |
| `duration` | string  | Yes         | -            | How long program runs                          |
| `type`     | string  | No          | `"playlist"` | Program type: `"playlist"` or `"liveset"`      |
| `playlist` | string  | Conditional | -            | M3U playlist path (required for playlist type) |
| `genres`   | array   | Conditional | -            | Genre list (required for liveset type)         |

### Details

#### `name`

A descriptive name for the program, used in logs and status displays.

- **Examples**: `"Morning Show"`, `"Techno Night"`, `"Jazz Hour"`

#### `active`

Whether this program is enabled for scheduling.

- **Values**:
    - `true` - Program will run according to schedule
    - `false` - Program is disabled (config preserved but not scheduled)
- **Use case**: Temporarily disable programs without deleting configuration

#### `cron`

A cron expression defining when the program should start.

- **Format**: `"minute hour day month weekday"`
- **Examples**:
    - `"0 6 * * 1-5"` - 6:00 AM Monday through Friday
    - `"0 19 * * *"` - 7:00 PM every day
    - `"0 22 * * 5,6"` - 10:00 PM on Friday and Saturday
    - `"0 14 * * 6"` - 2:00 PM on Saturdays
    - `"*/30 * * * *"` - Every 30 minutes
- **Validation**: Cron expression is validated on startup; invalid expressions cause program to be skipped

#### `duration`

How long the program should run before returning to regular library playback.

- **Format**: Time string with single unit only
- **Valid units**: `m` (minutes) or `h` (hours)
- **Examples**:
    - `"30m"` - 30 minutes
    - `"2h"` - 2 hours
    - `"45m"` - 45 minutes
    - `"90m"` - 90 minutes (1.5 hours)
- **IMPORTANT**: Combined formats like `"1h30m"` are NOT supported. Use minutes only for such durations (e.g., `"90m"`).

#### `type`

The type of scheduled program.

- **Values**:
    - `"playlist"` (default) - Plays tracks from local M3U playlist file
    - `"liveset"` - Fetches and streams livesets from hearthis.at API
- **Behavior**: If not specified, defaults to `"playlist"`

#### `playlist`

Path to an M3U playlist file (required for playlist programs).

- **Format**: Absolute or relative file path
- **File format**: Standard M3U playlist with audio file paths
- **Validation**: File existence and format validated on program activation
- **Example**: `"/home/radio/playlists/morning.m3u"`

#### `genres`

Array of music genres to fetch from hearthis.at (required for liveset programs).

- **Format**: Array of genre strings (case-insensitive)
- **Empty array behavior**: Use `[]` to fetch from general feed (all genres)
- **Fallback**: If no tracks found in specified genres, falls back to general feed
- **Examples**:
    - `["techno", "house"]`
    - `["deephouse", "progressivehouse"]`
    - `[]` (general feed)

### Available Hearthis.at Genres

When using liveset programs, you can specify any of these genre tags (case-insensitive, spaces converted to hyphens):

#### Electronic/Dance

`techno`, `house`, `deephouse`, `progressivehouse`, `techhouse`, `trance`, `psytrance`, `dubstep`, `dub`, `dubtechno`,
`drumandbass`, `jungle`, `breakbeat`, `breakcore`, `hardcore`, `hardstyle`, `edm`, `electro`, `electonica`,
`futurebass`, `idm`, `organichouse`, `garage`, `bass`, `dance`

#### Hip Hop/Urban

`hiphop`, `trap`, `urban`, `dancehall`

#### Pop/Rock

`pop`, `rock`, `indie`, `punk`, `heavymetal`, `industrial`

#### Soul/Funk

`soul`, `funk`, `rnb`, `disco`

#### Acoustic/Instrumental

`acoustic`, `instrumental`, `songwriter`, `folk`, `country`, `blues`

#### Ambient/Chill

`ambient`, `chillout`, `downtempo`, `lofi`

#### World

`bollywood`, `indian`, `reggae`, `world`, `amapiano`

#### Other

`jazz`, `classical`, `orchestral`, `experimental`, `soundart`, `spiritual`, `podcast`, `audiobook`, `radioshow`,
`clubs`, `festival`, `soundtrack`, `livestreams`, `replays`, `other`

### Playlist Program Examples

#### Weekday Morning Show

```toml
[[schedule.programs]]
name = "Morning Show"
active = true
cron = "0 6 * * 1-5"  # 6 AM, Monday-Friday
duration = "180m"  # 3 hours
type = "playlist"
playlist = "/path/to/playlists/morning.m3u"
```

#### Evening Jazz

```toml
[[schedule.programs]]
name = "Evening Jazz"
active = true
cron = "0 19 * * *"  # 7 PM daily
duration = "2h"
type = "playlist"
playlist = "/path/to/playlists/jazz.m3u"
```

#### Weekend Special

```toml
[[schedule.programs]]
name = "Weekend Mix"
active = true
cron = "0 10 * * 6,0"  # 10 AM Saturday and Sunday
duration = "4h"
type = "playlist"
playlist = "/path/to/playlists/weekend.m3u"
```

### Liveset Program Examples

#### Friday Night Techno

```toml
[[schedule.programs]]
name = "Techno Night"
active = true
cron = "0 22 * * 5,6"  # 10 PM Friday and Saturday
duration = "4h"
type = "liveset"
genres = ["techno", "techhouse", "dubtechno"]
```

#### Sunday Morning Chill

```toml
[[schedule.programs]]
name = "Electronic Morning"
active = true
cron = "0 8 * * 0"  # 8 AM Sundays
duration = "2h"
type = "liveset"
genres = ["ambient", "chillout", "downtempo"]
```

#### Deep House Session

```toml
[[schedule.programs]]
name = "Deep House Session"
active = true
cron = "0 14 * * 6"  # 2 PM Saturdays
duration = "3h"
type = "liveset"
genres = ["deephouse", "house", "organichouse"]
```

#### General Electronic Feed

```toml
[[schedule.programs]]
name = "Electronic Mix"
active = true
cron = "0 20 * * *"  # 8 PM daily
duration = "2h"
type = "liveset"
genres = []  # Empty array = fetch from all genres
```

### Schedule Behavior

- Programs only run when `active = true`
- When a program starts, it interrupts current playback
- When a program ends, playback returns to the main library
- Multiple programs can be scheduled at different times
- If programs overlap, the most recently started program takes priority
- Invalid programs (bad cron, missing files, etc.) are logged and skipped

## M3U Playlist Format

Funkstrom supports standard M3U and Extended M3U playlist formats for scheduled programs.

### Format Specification

- **File extension:** `.m3u` or `.m3u8`
- **Encoding:** UTF-8 recommended
- **Line format:** One file path per line
- **Comments:** Lines starting with `#` are ignored
- **Empty lines:** Ignored

### Path Resolution

1. **Absolute paths:** Used as-is
   ```
   /home/user/music/track1.mp3
   /var/radio/shows/episode1.mp3
   ```

2. **Relative paths:** Resolved from playlist file's directory
   ```
   # If playlist is at /playlists/morning.m3u
   ../music/track1.mp3          # Resolves to /music/track1.mp3
   songs/track2.mp3             # Resolves to /playlists/songs/track2.mp3
   ```

### Extended M3U Format

Metadata lines (#EXTINF) are parsed but currently ignored:

```m3u
#EXTM3U
#EXTINF:123,Artist Name - Track Title
/path/to/track1.mp3
#EXTINF:234,Another Artist - Another Track
/path/to/track2.mp3
```

### Error Handling

- **Missing files:** Skipped with warning logged, playlist continues
- **Empty playlist:** Error on startup, program not activated
- **Invalid playlist path:** Error on startup, program not activated

### Example Playlist

```m3u
# Morning Show Playlist
# Comments are allowed

# Absolute paths
/home/radio/music/intro.mp3
/home/radio/jingles/morning-jingle.mp3

# Relative paths (relative to playlist location)
tracks/song1.mp3
tracks/song2.mp3
../shared/outro.mp3

# Extended M3U metadata (optional)
#EXTINF:180,Artist - Song Title
tracks/song3.mp3
```

### Creating Playlists

```bash
# Simple playlist with find
find /home/radio/music/morning -name "*.mp3" > morning.m3u

# Extended M3U with metadata (manual creation)
cat > morning.m3u << 'EOF'
#EXTM3U
#EXTINF:-1,Morning Intro
/home/radio/music/intro.mp3
EOF
```

## HTTP API Reference

Funkstrom exposes several HTTP endpoints for streaming and monitoring.

### Endpoints

| Endpoint         | Method | Description                               | Content Type                    |
|------------------|--------|-------------------------------------------|---------------------------------|
| `/<stream_name>` | GET    | Audio stream (e.g., `/high`, `/standard`) | `audio/mpeg`, `audio/aac`, etc. |
| `/status`        | GET    | Server status and buffer information      | `application/json`              |
| `/current`       | GET    | Currently playing track metadata          | `application/json`              |
| `/`              | GET    | Station info page with stream links       | `text/html`                     |
| `/swagger`       | GET    | Swagger UI for API documentation          | `text/html`                     |
| `/openapi.json`  | GET    | OpenAPI specification                     | `application/json`              |

### Stream Endpoint

**URL:** `GET /<stream_name>`

Returns an audio stream configured in your `config.toml`.

**Example:**

```bash
# Listen with mpv
mpv http://localhost:8284/high

# Listen with curl and play
curl http://localhost:8284/standard | ffplay -

# Open in VLC
vlc http://localhost:8284/mobile
```

### Status Endpoint

**URL:** `GET /status`

Returns JSON with server status and buffer information.

**Response Example:**

```json
{
  "streams": [
    {
      "name": "high",
      "status": "running",
      "buffer_size": 524288,
      "listeners": 3
    }
  ],
  "current_track": {
    "title": "Track Title",
    "artist": "Artist Name",
    "album": "Album Name"
  }
}
```

**Example:**

```bash
curl http://localhost:8284/status | jq .
```

### Current Track Endpoint

**URL:** `GET /current`

Returns JSON with currently playing track metadata.

**Response Example:**

```json
{
  "title": "Track Title",
  "artist": "Artist Name",
  "album": "Album Name",
  "duration": 245,
  "file_path": "/music/track.mp3"
}
```

**Example:**

```bash
curl http://localhost:8284/current | jq .
```

### Info Page

**URL:** `GET /`

Returns an HTML page with station information and links to all available streams.

**Example:**

```bash
# Open in browser
xdg-open http://localhost:8284/
```

### API Documentation

**Swagger UI:** `GET /swagger`
**OpenAPI Spec:** `GET /openapi.json`

Interactive API documentation with request/response examples.

## Database

Funkstrom uses SQLite to index your music library for fast access and persistence.

### Location and Structure

- **Database file:** `./data/database.db` (relative to working directory)
- **Auto-creation:** Database and schema created automatically on first run
- **Permissions:** Requires write access to `./data/` directory
- **Data stored:** Track metadata (title, artist, album), file paths, modification times, shuffle state
- **Persistence:** Library scan results and playback state persist across restarts

### Initial Setup

The first time you start Funkstrom, it will:

1. Create `./data/` directory if it doesn't exist
2. Create `database.db` and initialize schema
3. Perform full library scan of your music directory
4. Index all tracks with metadata (may take several minutes for large libraries)

### Library Scanning

Funkstrom automatically manages your library index:

- **First startup:** Full scan (reads all audio files)
- **Subsequent startups:** Incremental scan (only checks for changes)
- **Detection:** Automatically detects added, modified, and deleted tracks

### Rescanning Library

To force a complete rescan of your music library:

```bash
# 1. Stop the server
pkill funkstrom

# 2. Delete the database
rm -rf ./data/database.db

# 3. Restart (will perform full scan)
RUST_LOG=info cargo run -- --config config.toml
```

### Database Management

- **Backup:** Copy `./data/database.db` to safe location
- **Restore:** Copy backup back to `./data/database.db` and restart server
- **Reset:** Delete database file to start fresh

## Frequently Asked Questions

### Can I change configuration without restarting?

No. Configuration changes require a server restart. There is no hot-reload support currently.

### How do I add new music to the library?

1. Copy music files to your `music_directory`
2. Restart the server (it will automatically detect new files during incremental scan)
3. Or delete database and restart for a full rescan

### Can I run multiple instances of Funkstrom?

Yes, but each instance requires:

- Unique port number
- Unique database path (different working directories)
- Separate music directories (or read-only shared access)

Example:

```bash
# Instance 1
cd /home/radio/instance1
RUST_LOG=info ./funkstrom --config config.toml

# Instance 2
cd /home/radio/instance2
RUST_LOG=info ./funkstrom --config config.toml
```

### What happens when a scheduled program starts mid-track?

The current track is interrupted immediately and the scheduled program begins.

### What happens when a scheduled program ends mid-track?

The program track finishes playing, then playback returns to the main library.

### Can I stream from URLs instead of local files?

Yes! The audio processor supports HTTP/HTTPS URLs:

- Include URLs in M3U playlists: `https://example.com/track.mp3`
- Used by the liveset feature to stream from hearthis.at
- Works for any HTTP-accessible audio file

### Why is my first startup so slow?

The initial library scan reads metadata from every audio file, which takes time. For reference:

- 1,000 tracks: ~30 seconds
- 10,000 tracks: ~5 minutes
- 50,000 tracks: ~20-30 minutes

Subsequent startups use incremental scanning and are much faster (seconds).

### Can I schedule the same program multiple times per day?

Yes! Use a cron expression that runs multiple times:

```toml
[[schedule.programs]]
name = "Hourly News"
cron = "0 * * * *"  # Every hour on the hour
duration = "5m"
type = "playlist"
playlist = "/playlists/news.m3u"
```

Or define the same program multiple times with different cron schedules.

### What audio formats are supported in the music library?

Any format FFmpeg can decode, including:

- **Common:** MP3, FLAC, OGG, M4A, WAV, AAC
- **Less common:** WMA, OPUS, APE, ALAC, WV
- **Requirements:** FFmpeg must have codec support

Your library can be in any format - FFmpeg transcodes to the stream format.

### How do I limit the number of listeners?

Funkstrom doesn't have built-in listener limits.

### Can I password-protect my streams?

Not natively.

### How do I monitor listener count?

Use the `/status` endpoint:

```bash
curl http://localhost:8284/status | jq '.streams[].listeners'
```

## Complete Examples

### Minimal Configuration

Bare minimum configuration for a simple local radio station:

```toml
[server]
port = 8284
bind_address = "127.0.0.1"

[library]
music_directory = "/home/user/Music"
shuffle = true
repeat = true

[station]
station_name = "My Radio"
description = "Personal radio station"
genre = "Various"
url = "http://localhost:8284"

[stream.default]
bitrate = 128
format = "mp3"
sample_rate = 44100
channels = 2
enabled = true
```

### Multi-Stream Configuration

Production setup with multiple quality options:

```toml
[server]
port = 8284
bind_address = "0.0.0.0"
ffmpeg_path = "/usr/bin/ffmpeg"

[library]
music_directory = "/var/radio/music"
shuffle = true
repeat = true

[station]
station_name = "Deep Sea Radio"
description = "Electronic music from the depths"
genre = "Electronic"
url = "https://radio.example.com"

# High quality for desktop listeners
[stream.high]
bitrate = 320
format = "mp3"
sample_rate = 48000
channels = 2
enabled = true

# Standard quality for most listeners
[stream.standard]
bitrate = 128
format = "mp3"
sample_rate = 44100
channels = 2
enabled = true

# Mobile-optimized stream
[stream.mobile]
bitrate = 48
format = "aac"
sample_rate = 22050
channels = 1
enabled = true
```

### Full Configuration with Scheduling

Complete setup with multiple streams and scheduled programming:

```toml
[server]
port = 8284
bind_address = "0.0.0.0"
ffmpeg_path = "/usr/bin/ffmpeg"

[library]
music_directory = "/var/radio/music"
shuffle = true
repeat = true

[station]
station_name = "Deep Sea Radio"
description = "Electronic music and curated shows"
genre = "Electronic"
url = "https://radio.example.com"

[stream.high]
bitrate = 320
format = "mp3"
sample_rate = 48000
channels = 2
enabled = true

[stream.standard]
bitrate = 128
format = "mp3"
sample_rate = 44100
channels = 2
enabled = true

[stream.mobile]
bitrate = 48
format = "aac"
sample_rate = 22050
channels = 1
enabled = true

# Weekday morning show (3 hours)
[[schedule.programs]]
name = "Morning Drive"
active = true
cron = "0 7 * * 1-5"
duration = "180m"
type = "playlist"
playlist = "/var/radio/playlists/morning.m3u"

# Weekend techno nights
[[schedule.programs]]
name = "Techno Night"
active = true
cron = "0 22 * * 5,6"
duration = "4h"
type = "liveset"
genres = ["techno", "techhouse", "dubtechno"]

# Sunday chill sessions
[[schedule.programs]]
name = "Sunday Chill"
active = true
cron = "0 10 * * 0"
duration = "3h"
type = "liveset"
genres = ["ambient", "chillout", "downtempo"]
```
