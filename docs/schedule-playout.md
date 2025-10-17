# Schedule Playout Feature

## Overview

The Schedule Playout feature allows you to interrupt the default library playout with scheduled programs at specific times using cron syntax. Programs play M3U playlists for a defined duration, then automatically return to the default library playout.

## Behavior

- **Default playout**: The `[library]` section runs 24/7 as the foundation
- **Scheduled programs**: Interrupt the default library when active
- **After program ends**: Automatically returns to default library playout
- **Between programs**: Default library fills all gaps
- **Automatic activation**: Schedule engine activates if any program has `active = true`

## Configuration

### Basic Structure

```toml
# Default playout - runs 24/7 unless interrupted
[library]
music_directory = "/path/to/default/music"
shuffle = true
repeat = true

# Scheduled programs
[[schedule.programs]]
name = "Morning Show"
active = true
cron = "0 6 * * 1-5"
duration = "3h"
playlist = "/playlists/morning.m3u"

[[schedule.programs]]
name = "Evening Jazz"
active = true
cron = "0 19 * * *"
duration = "2h"
playlist = "/playlists/jazz.m3u"
```

### Program Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Program identifier for logging |
| `active` | Boolean | Yes | Enable/disable this program |
| `cron` | String | Yes | Cron expression for start time |
| `duration` | String | Yes | Duration in human format (30m, 2h) |
| `playlist` | String | Yes | Path to M3U playlist file |

### Duration Format

Supported formats:
- **Minutes**: `"30m"`, `"45m"`, `"90m"`
- **Hours**: `"1h"`, `"2h"`, `"12h"`

Examples:
```toml
duration = "30m"   # 30 minutes
duration = "2h"    # 2 hours
duration = "90m"   # 1 hour 30 minutes
```

### Cron Expressions

Standard cron syntax with 5 fields:
```
┌─────── minute (0 - 59)
│ ┌───── hour (0 - 23)
│ │ ┌─── day of month (1 - 31)
│ │ │ ┌─ month (1 - 12)
│ │ │ │ ┌ day of week (0 - 6) (Sunday = 0)
│ │ │ │ │
* * * * *
```

Examples:
```toml
cron = "0 6 * * *"      # Every day at 6:00 AM
cron = "0 6 * * 1-5"    # Weekdays at 6:00 AM
cron = "0 19 * * *"     # Every day at 7:00 PM
cron = "0 12 * * 6,0"   # Weekends at noon
cron = "30 8 * * 1"     # Mondays at 8:30 AM
```

## M3U Playlist Format

Programs use M3U playlist files to define track lists.

### Extended M3U Format

```m3u
#EXTM3U
#EXTINF:180,Artist - Song Title 1
/path/to/music/song1.mp3
#EXTINF:240,Artist - Song Title 2
/path/to/music/song2.mp3
#EXTINF:210,Artist - Song Title 3
/path/to/music/song3.mp3
```

### Simple M3U Format

```m3u
/music/jazz/track01.mp3
/music/jazz/track02.mp3
/music/jazz/track03.mp3
/music/jazz/track04.mp3
```

### Path Support

- **Absolute paths**: `/music/jazz/track.mp3`
- **Relative paths**: `../music/track.mp3` (relative to M3U file location)
- **Comments**: Lines starting with `#` (except `#EXTM3U` and `#EXTINF`)

## Complete Configuration Example

```toml
[server]
port = 8284
bind_address = "127.0.0.1"

[library]
music_directory = "/music/library"
shuffle = true
repeat = true

[stream]
station_name = "My Radio Station"
description = "Great music 24/7"
genre = "Various"
url = "http://localhost:8284"
bitrate = 128
format = "mp3"
sample_rate = 44100
channels = 2

[ffmpeg]
path = "/usr/bin/ffmpeg"

# Morning show on weekdays
[[schedule.programs]]
name = "Morning Show"
active = true
cron = "0 6 * * 1-5"
duration = "3h"
playlist = "/playlists/morning.m3u"

# Lunch music every day
[[schedule.programs]]
name = "Lunch Hour"
active = true
cron = "0 12 * * *"
duration = "90m"
playlist = "/playlists/lunch.m3u"

# Evening jazz daily
[[schedule.programs]]
name = "Evening Jazz"
active = true
cron = "0 19 * * *"
duration = "2h"
playlist = "/playlists/jazz.m3u"

# Late night ambient
[[schedule.programs]]
name = "Late Night"
active = true
cron = "0 23 * * *"
duration = "7h"
playlist = "/playlists/ambient.m3u"

# Disabled program (for testing)
[[schedule.programs]]
name = "Weekend Special"
active = false
cron = "0 10 * * 6,0"
duration = "4h"
playlist = "/playlists/weekend.m3u"
```

## Playout Timeline Example

Based on the configuration above:

```
00:00 - 06:00  → Library (/music/library) - Default playout
06:00 - 09:00  → Morning Show (morning.m3u) - Scheduled program
09:00 - 12:00  → Library - Default playout
12:00 - 13:30  → Lunch Hour (lunch.m3u) - Scheduled program
13:30 - 19:00  → Library - Default playout
19:00 - 21:00  → Evening Jazz (jazz.m3u) - Scheduled program
21:00 - 23:00  → Library - Default playout
23:00 - 06:00  → Late Night (ambient.m3u) - Scheduled program
```

## Behavior Details

### Shuffle and Repeat

Programs inherit the `shuffle` and `repeat` settings from the `[library]` section:
- If `shuffle = true`, program playlists are shuffled
- If `repeat = true`, program playlists loop if they finish before duration ends

### Transitions

- Transitions occur **between tracks**, not mid-track
- Current track finishes before switching to next program/library
- Smooth handoff ensures no audio gaps

### Program Duration

- Programs run for the specified duration, then stop
- If playlist ends before duration: behavior depends on `repeat` setting
  - `repeat = true`: Playlist loops
  - `repeat = false`: Returns to library early
- If playlist is longer than duration: Stops at duration end

### Activation Logic

```
If any program has active = true:
    → Schedule engine activates
    → Monitors time and switches playlists
Else:
    → Pure library mode (no scheduling overhead)
```

## Validation

At startup, the system validates:

1. **M3U file exists** for all active programs
2. **Cron expressions are valid** syntax
3. **Duration format is correct** (digits + m/h)
4. **Audio files in M3U exist** (warnings logged for missing files)

Invalid programs are skipped with error messages in logs.

## Logging

Schedule events are logged:
```
INFO: Schedule engine activated (3 active programs)
INFO: Program started: Morning Show (morning.m3u)
INFO: Program ended: Morning Show, returning to library
WARN: Program "Test Show" skipped: playlist file not found
ERROR: Invalid cron expression in "Bad Program": ...
```

## Disabling Schedule

To disable all scheduling and use pure library mode:

1. Set all programs to `active = false`, or
2. Remove all `[[schedule.programs]]` sections

The system will operate in traditional library-only mode.

## Implementation Dependencies

Required Rust crates:
```toml
cron = "0.12"      # Cron expression parsing
chrono = "0.4"     # Time/duration handling
m3u = "1.0"        # M3U playlist parsing
```

## Architecture

```
┌─────────────────────────────────────────────┐
│         Schedule Engine (if active)         │
│  - Monitors time                            │
│  - Calculates program boundaries            │
│  - Sends playlist switch commands           │
└─────────────┬───────────────────────────────┘
              │ playlist_switch_tx
              ▼
┌─────────────────────────────────────────────┐
│          AudioReader (enhanced)             │
│  - Default: plays [library] directory       │
│  - On command: loads M3U playlist           │
│  - After duration: returns to library       │
└─────────────┬───────────────────────────────┘
              │ track_rx
              ▼
┌─────────────────────────────────────────────┐
│      FFmpegProcessor (unchanged)            │
└─────────────────────────────────────────────┘
```

## Use Cases

### Radio Station with Shows

Traditional radio station with scheduled programming:
- Morning show (6-9 AM weekdays)
- Drive time (5-7 PM daily)
- Weekend programming
- Default music fills all other times

### Music Rotation by Time

Different music genres at different times:
- Energetic music in morning
- Relaxed music at lunch
- Ambient at night
- Regular rotation between scheduled blocks

### Testing and Development

Easy program management:
- Set `active = false` to disable programs during testing
- Enable one program at a time for validation
- No need to modify cron or remove configurations
