# Repository Guidelines

## Project Overview

Funkstrom is an Icecast-compatible internet radio server written in Rust. It scans a local music library, builds playlists, transcodes audio via FFmpeg, and streams to unlimited concurrent HTTP listeners with Icecast/Shoutcast protocol headers. Supports scheduled programming (cron-based playlist and liveset switching) and integration with the Hearthis.at API.

## Architecture & Data Flow

```
CLI (clap) → Config (TOML) → Library Scanner → SQLite DB (rusqlite + r2d2)
                                    ↓
                            AudioReader (DB → playlist)
                                    ↓
                     Schedule Engine (cron) → PlaylistCommand (crossbeam)
                                    ↓
                          PlaybackDirector (timeline, watch::channel)
                                    ↓
                          FFmpegProcessor (transcoding subprocess)
                                    ↓
                           FanoutBuffer (ring buffer, Arc<RwLock<>>)
                                    ↓
                    Icecast Server (warp) → HTTP listeners (chunked transfer)
```

**Key data flow**: Tracks are indexed from disk into SQLite. `AudioReader` builds optionally-shuffled playlists and feeds them to `PlaybackDirector`, which advances through tracks by wall-clock time, broadcasting `TimelineSnapshot` via `tokio::watch`. `FFmpegProcessor` spawns an FFmpeg subprocess to transcode the current track to the configured output format; audio chunks (8KB) are pushed into a shared `FanoutBuffer`. HTTP listeners each hold an independent cursor into the ring buffer, receiving chunks via an `mpsc` unbounded channel.

**Schedule engine** runs as a `tokio::spawn`'d async loop, evaluating cron expressions and sending `PlaylistCommand` variants (`SwitchToPlaylist`, `SwitchToLiveset`, `ReturnToLibrary`) through a `crossbeam-channel` to `AudioReader`, which mutates the `PlaybackDirector` via `Arc<Mutex<>>`.

## Key Directories

| Directory         | Purpose                                                  |
| ----------------- | -------------------------------------------------------- |
| `src/`            | All Rust source files — flat structure, no submodules    |
| `e2e/`            | End-to-end test scripts (bash + curl + jq)               |
| `docs/`           | User-facing documentation (configuration.md, general.md) |
| `container-data/` | Default config baked into Docker images                  |

## Development Commands

```bash
# Build
cargo build
cargo build --release          # standard release (debug symbols)
cargo build --profile production  # optimized: LTO, codegen-units=1, stripped

# Run (application blocks until killed)
cargo run -- --config config.toml

# Test
cargo test                      # all unit tests
cargo test <test_name>          # specific test
./e2e/test.sh                   # 16 E2E tests (requires running server on port 3002)
./e2e/test_sync.sh              # 4 multi-listener sync tests (self-manages server)

# Lint & Format
cargo clippy
cargo fmt
```

## Code Conventions & Common Patterns

### Error Handling

- Functions return `Result<T, Box<dyn std::error::Error + Send + Sync>>` for fallible operations.
- Avoid bare `.expect()` / `.unwrap()` in production paths — prefer `?` propagation.

### Async Runtime

- `#[tokio::main]` entry point with `features = ["full"]`.
- Use `tokio::spawn` for background tasks; `tokio::select!` for awaiting termination.
- Blocking I/O (file reads, FFmpeg subprocess) runs on dedicated threads, communicating via channels.

### Channel Strategy

- **crossbeam-channel** for thread-to-async communication (e.g., schedule commands → AudioReader).
- **tokio::sync::watch** for one-to-many broadcasts (e.g., `PlaybackDirector` → HTTP server for `/current` metadata).
- **tokio::sync::mpsc** (unbounded) for per-listener audio chunk delivery from `FanoutBuffer`.

### Shared State

- `Arc<RwLock<T>>` for read-heavy shared state (`FanoutBuffer`).
- `Arc<Mutex<T>>` for mutation-heavy shared state (`PlaybackDirector`).
- `Arc<Vec<T>>` for immutable shared configuration.

### Naming

- `snake_case` for variables, functions, modules.
- `PascalCase` for structs, enums, type aliases.
- Files are named in `snake_case`.

### Logging

- Use the `log` crate (`info!`, `warn!`, `error!`, `debug!`) with `env_logger`.
- Log messages are structured and include contextual detail.

### Configuration

- TOML format, deserialized via `serde` into typed structs in `config.rs`.
- `Config::validate()` called at startup; panics on invalid config (fail-fast).
- Stream names: alphanumeric plus underscore/hyphen only.

### Testing Patterns

- Unit tests are inline in source files using `#[cfg(test)]` modules.
- Use `tempfile` crate for filesystem isolation in tests.
- E2E tests use bash scripts with an accumulator pattern (`P=0; F=0`), curl for HTTP, jq for JSON assertions.
- CI config at `e2e/ci-config.toml` disables shuffle, uses port 3002.

## Important Files

| File                       | Role                                                                                                                                |
| -------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `src/main.rs`              | Entry point — module declarations, `StreamPipeline` struct, full startup sequence                                                   |
| `src/config.rs`            | TOML config model — `Config`, `ServerConfig`, `LibraryConfig`, `StationConfig`, `StreamConfig`, `ScheduleConfig`, `ScheduleProgram` |
| `src/cli.rs`               | CLI parser — single `-c/--config` option, default `./config.toml`                                                                   |
| `src/audio_processor.rs`   | FFmpeg subprocess wrapper — format mapping, transcoding, idle detection                                                             |
| `src/audio_reader.rs`      | Playlist builder + schedule command service                                                                                         |
| `src/playback_director.rs` | Timeline engine — track advancement, snapshot broadcasting                                                                          |
| `src/fanout_buffer.rs`     | Ring buffer — chunk storage, per-listener cursors, eviction                                                                         |
| `src/server_icecast.rs`    | Warp HTTP server — Icecast routes, listener tracking, info page                                                                     |
| `src/server_swagger.rs`    | Swagger UI + OpenAPI spec routes                                                                                                    |
| `src/schedule_engine.rs`   | Cron-based scheduling — program validation, next-event calculation                                                                  |
| `src/library_scanner.rs`   | Audio file discovery — full and incremental scans, ffprobe duration                                                                 |
| `src/library_db.rs`        | SQLite layer — rusqlite + r2d2 pool, WAL mode, batch operations                                                                     |
| `src/hearthis_client.rs`   | Hearthis.at API client — feed/genre endpoints, random track selection                                                               |
| `src/m3u_parser.rs`        | M3U playlist parser — relative/absolute paths, validation                                                                           |
| `src/audio_metadata.rs`    | Track metadata extraction — audiotags + ICY/JSON formatting                                                                         |
| `config.toml.example`      | Annotated example with all options documented                                                                                       |
| `Cargo.toml`               | Dependencies, `[profile.production]` for optimized builds                                                                           |
| `Containerfile`            | Multi-stage Docker build (FFmpeg static + Rust musl-cross → scratch)                                                                |

## Runtime & Tooling Preferences

- **Rust edition**: 2021
- **Async runtime**: tokio (full features)
- **HTTP framework**: warp 0.3
- **Database**: SQLite via rusqlite 0.37 (bundled) with r2d2 connection pool (max_size=5)
- **Templating**: minijinja 2.12 (info page HTML)
- **External dependency**: FFmpeg binary required at runtime (path configurable via `ffmpeg_path` in config or `FFMPEG_PATH` env var)
- **Allocator**: mimalloc
- **Containerization**: Multi-stage Docker build targeting `scratch` with static musl binary
  CI
- `e2e/ci-config.toml` provides deterministic test configuration (shuffle off, repeat on, fixed port 3002).

### Technical Requirements

- External crates are allowed, but keep them as low as possible
- Prefer standard Rust libraries and built-in features to minimize external package usage.
- Evaluate trade-offs before adding any third-party crate.
- When using external crates, make sure to use the very latest stable versions.
- All static files needs to be embedded into the binary
- Must compile and run without errors
- Handle user interactions gracefully
- Implement proper error handling and validation
- Use appropriate Rust idioms and patterns
- Logging: prefer `tracing`/`tracing_subscriber` with contextual spans instead of `println!`.
- Error handling: avoid `unwrap`/`expect` in non-test code; surface actionable errors to the UI.
- Structure code into small, focused rust files without using rust modules
- Each file should encapsulate a single responsibility or closely related functionalities.
- Promote reusability and ease of testing by isolating components.
- Follow the SOLID object-oriented design principles to ensure maintainable and extensible code.
- Emphasize single responsibility, open-closed, Liskov substitution, interface segregation, and dependency inversion
  where applicable.
- Use descriptive names and avoid clever tricks or shortcuts that hinder comprehensibility.
- YAGNI - You Aren't Gonna Need It: Avoid adding functionality until it is necessary.
- Don't write unused code for future features.
- Always run code formatters (`cargo fmt`) and linters (`cargo clippy`) when finishing a task.
- Maintain consistent code style across the project to improve readability and reduce friction in reviews.
- Always use RustTLS for any TLS connections, no OpenSSL.
- When planning a new feature, do not only rely on your training data, also get a second opinion via the web search.

## Testing & QA

### Unit Tests

- Co-located with source in `#[cfg(test)]` modules.
- Run with `cargo test`.
- Use `tempfile` for temporary directories and files.

### E2E Tests

- Bash scripts in `e2e/`.
- `test.sh` (16 tests): requires a pre-started server on port 3002 with `ci-config.toml`. Covers status endpoint, streaming, ICY headers, listener counting, multi-format streams.
- `test_sync.sh` (4 tests): self-manages server lifecycle. Covers same-stream sync delivery, idle timeline advancement, metadata persistence across idle periods.
- Both use curl for HTTP requests and jq for JSON assertions.

### Test-Driven Development (TDD)

- Prefer write tests before writing the functionality.
- Use tests to drive design decisions and ensure robust feature implementation.

### Behavior-Driven Development (BDD)

- Write tests in a BDD style, focusing on the expected behavior and outcomes.
- Structure tests to clearly state scenarios, actions, and expected results to improve communication and documentation.
