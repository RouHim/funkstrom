![Funkstrom Banner](.github/readme/banner.svg)

[![CI](https://github.com/RouHim/funkstrom/actions/workflows/ci.yml/badge.svg)](https://github.com/RouHim/funkstrom/actions/workflows/ci.yml)
[![Donate me](https://img.shields.io/badge/-buy_me_a%C2%A0coffee-gray?logo=buy-me-a-coffee)](https://buymeacoffee.com/rouhim)

*A fast, lightweight Icecast-compatible internet radio server written in Rust.*

## Motivation

I wanted a simple, self-hosted radio streaming solution that could handle continuous audio streaming with automatic transcoding, metadata support, and Icecast compatibility—all while being fast and resource-efficient.

## How it works

Funkstrom reads audio files from your configured music directory, transcodes them to MP3 using FFmpeg, and streams them continuously to connected clients via the Icecast protocol. It maintains a circular buffer for smooth playback, extracts metadata from audio files, and serves multiple clients simultaneously. The server provides a web interface for monitoring, current track information, and API endpoints for integration.

## Features

- **Icecast Streaming**: Full Icecast protocol support for compatibility with VLC, iTunes, Winamp, and more
- **Automatic Transcoding**: Converts audio to MP3 via FFmpeg for universal compatibility
- **Metadata Extraction**: Reads ID3 tags and other metadata from audio files
- **Circular Buffer**: Smooth continuous playback without gaps
- **Multiple Clients**: Serve unlimited simultaneous listeners
- **Web Interface**: Built-in status page and API documentation
- **REST API**: JSON endpoints for status, metadata, and monitoring
- **Low Resource Usage**: Written in Rust for performance and efficiency
- **Multiple Format Support**: MP3, FLAC, WAV, OGG, AAC, M4A, OPUS, WMA

## Run the application

### Docker

Docker Example:

```bash
docker run -p 3002:3002 \
        -v /path/to/music:/music:ro \
        rouhim/funkstrom
```

Docker compose example:

```yaml
services:
  funkstrom:
    image: rouhim/funkstrom
    volumes:
      - /path/to/music:/music:ro
    ports:
      - "3002:3002"
    environment:
      - RUST_LOG=info
```

### Native execution

#### Prerequisites

- Rust 1.70+ (for building from source)
- FFmpeg

#### Installation

Download the latest release for your system from the [releases page](https://github.com/RouHim/funkstrom/releases):

```bash
# Assuming you run a x86/x64 system, if not adjust the binary name to download
LATEST_VERSION=$(curl -L -s -H 'Accept: application/json' https://github.com/RouHim/funkstrom/releases/latest | \
sed -e 's/.*"tag_name":"\([^"]*\)".*/\1/') && \
curl -L -o funkstrom https://github.com/RouHim/funkstrom/releases/download/$LATEST_VERSION/funkstrom-x86_64-unknown-linux-musl && \
chmod +x funkstrom
```

Or build from source:

```bash
cargo build --release
```

#### Running

Create a configuration file `config.toml`:

```toml
[server]
port = 3002
bind_address = "127.0.0.1"

[audio]
music_directory = "/path/to/music"
shuffle = true
repeat = true
bitrate = 128

[station]
name = "My Radio Station"
description = "Great music 24/7"
genre = "Various"
```

Start the server:

```bash
./funkstrom --config config.toml
```

Then open your browser at `http://127.0.0.1:3002/` or stream audio at `http://127.0.0.1:3002/stream`

## Configuration

All configuration is done via the `config.toml` file:

| Section | Key | Description | Default | Required |
|---------|-----|-------------|---------|----------|
| `[server]` | `port` | Port on which the server should listen | `3002` | No |
| `[server]` | `bind_address` | IP address to bind to | `127.0.0.1` | No |
| `[audio]` | `music_directory` | Path to music directory | - | Yes |
| `[audio]` | `shuffle` | Shuffle playback order | `true` | No |
| `[audio]` | `repeat` | Repeat playlist when finished | `true` | No |
| `[audio]` | `bitrate` | MP3 bitrate in kbps | `128` | No |
| `[station]` | `name` | Station name (Icecast header) | `"My Radio Station"` | No |
| `[station]` | `description` | Station description | `"Great music 24/7"` | No |
| `[station]` | `genre` | Station genre | `"Various"` | No |

You can also set `RUST_LOG` environment variable for logging (trace, debug, info, warn, error).

## API Endpoints

- **`GET /`** - Web interface with station info and current track
- **`GET /stream`** - Audio stream endpoint (Icecast compatible)
- **`GET /status`** - JSON status including buffer info and station details
- **`GET /current`** - JSON metadata for currently playing track
- **`GET /api-docs`** - Interactive Swagger API documentation

## Supported Formats

Audio files are automatically transcoded to MP3 for streaming. Supported input formats:

- **MP3, FLAC, WAV, OGG, AAC, M4A, OPUS, WMA** and more (anything FFmpeg supports)

## Support

If you find Funkstrom useful, consider [buying me a coffee](https://buymeacoffee.com/rouhim)

## License

MIT
