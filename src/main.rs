mod audio_metadata;
mod audio_processor;
mod audio_reader;
mod cli;
mod config;
mod fanout_buffer;
mod hearthis_client;
mod library_db;
mod library_scanner;
mod m3u_parser;
mod playback_director;
mod schedule_engine;
mod server_icecast;
mod server_swagger;

use audio_processor::{AudioChunk, FFmpegProcessor};
use audio_reader::AudioReader;
use cli::get_config_path;
use config::Config;
use crossbeam_channel::{unbounded, Receiver};
use fanout_buffer::{FanoutBuffer, SharedFanoutBuffer};
use library_db::LibraryDatabase;
use library_scanner::LibraryScanner;
use playback_director::{PlaybackDirector, TimelineSnapshot};
use schedule_engine::{PlaylistCommand, ScheduleEngine};
use server_icecast::{IcecastServer, StreamBufferEntry};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};
use tokio::sync::{watch, Notify};
use tokio::task::JoinHandle;

// Avoid musl's default allocator due to lackluster performance
// https://nickb.dev/blog/default-musl-allocator-considered-harmful-to-performance
#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

type AudioPipeline = (Vec<StreamPipeline>, watch::Receiver<TimelineSnapshot>);

struct StreamPipeline {
    name: String,
    buffer: SharedFanoutBuffer,
    receiver: Receiver<AudioChunk>,
    bitrate: u32,
    format: String,
    listeners: Arc<AtomicUsize>,
    notify: Arc<Notify>,
    is_paused: Arc<AtomicBool>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    env_logger::init();
    std::fs::create_dir_all("./data")?;

    // Load config
    let config_path = get_config_path();
    let config = Config::from_file(&config_path)?;

    log_startup_info(&config);

    // Initialize components
    let (db, scanner) = initialize_library(&config)?;
    let schedule_rx = setup_schedule_engine(&config);
    let (stream_pipelines, timeline_rx) = setup_audio_pipeline(&config, db, schedule_rx)?;

    // Set up streaming buffers and buffer writers for each stream
    let mut buffer_writer_handles = Vec::new();
    let mut stream_buffers = Vec::new();

    for pipeline in stream_pipelines {
        let handle = start_buffer_writer(pipeline.buffer.clone(), pipeline.receiver);
        buffer_writer_handles.push(handle);

        stream_buffers.push((
            pipeline.name,
            pipeline.buffer,
            pipeline.bitrate,
            pipeline.format.clone(),
            pipeline.listeners,
            pipeline.notify,
            pipeline.is_paused,
        ));
    }

    // Start server
    let server_handle = start_server(&config, stream_buffers, timeline_rx);

    log_server_urls(&config);

    // Start nightly rescan task
    let nightly_rescan_handle = start_nightly_rescan(scanner);

    // Wait for all tasks to complete
    tokio::select! {
        _ = server_handle => log::error!("Icecast server stopped"),
        _ = async {
            for handle in buffer_writer_handles {
                if let Err(e) = handle.await {
                    log::error!("Buffer writer task failed: {}", e);
                }
            }
        } => log::error!("All buffer writers stopped"),
        _ = nightly_rescan_handle => log::error!("Nightly rescan stopped"),
    }

    Ok(())
}

fn initialize_library(
    config: &Config,
) -> Result<(LibraryDatabase, LibraryScanner), Box<dyn std::error::Error + Send + Sync>> {
    let db = LibraryDatabase::new("./data/database.db")?;
    db.initialize_schema()?;

    let music_dir = PathBuf::from(&config.library.music_directory);
    let scanner = LibraryScanner::new(music_dir.clone(), db.clone());

    let track_count = db.track_count()?;
    if track_count == 0 {
        log::info!("Empty library, performing initial full scan...");
        let result = scanner.full_scan()?;
        log::info!("Initial scan complete: {} tracks added", result.added);
        if !result.errors.is_empty() {
            log::warn!("Scan encountered {} errors", result.errors.len());
        }
    } else {
        log_last_scan_times(&db);

        log::info!("Performing incremental library scan...");
        let result = scanner.incremental_scan()?;
        if result.added > 0 || result.updated > 0 || result.deleted > 0 {
            log::info!(
                "Library changes: +{} ~{} -{} tracks",
                result.added,
                result.updated,
                result.deleted
            );
        }
    }

    Ok((db, scanner))
}

fn log_last_scan_times(db: &LibraryDatabase) {
    if let Ok(Some(last_full)) = db.get_metadata("last_full_scan") {
        if let Ok(timestamp) = last_full.parse::<i64>() {
            let datetime = chrono::DateTime::from_timestamp(timestamp, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "unknown".to_string());
            log::info!("Last full scan: {}", datetime);
        }
    }

    if let Ok(Some(last_incr)) = db.get_metadata("last_incremental_scan") {
        if let Ok(timestamp) = last_incr.parse::<i64>() {
            let datetime = chrono::DateTime::from_timestamp(timestamp, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "unknown".to_string());
            log::info!("Last incremental scan: {}", datetime);
        }
    }
}

fn setup_schedule_engine(config: &Config) -> Option<Receiver<PlaylistCommand>> {
    let schedule_config = config.schedule.as_ref()?;

    if schedule_config.programs.is_empty() || !schedule_config.programs.iter().any(|p| p.active) {
        log::info!("No active programs found, running in library-only mode");
        return None;
    }

    match ScheduleEngine::new(schedule_config.programs.clone()) {
        Ok(engine) => {
            let rx = engine.get_command_receiver();
            engine.start();
            Some(rx)
        }
        Err(e) => {
            log::warn!("Failed to initialize schedule engine: {}", e);
            log::info!("Running in library-only mode");
            None
        }
    }
}

fn setup_audio_pipeline(
    config: &Config,
    db: LibraryDatabase,
    schedule_rx: Option<Receiver<PlaylistCommand>>,
) -> Result<AudioPipeline, Box<dyn std::error::Error + Send + Sync>> {
    let music_dir = PathBuf::from(&config.library.music_directory);
    let audio_reader = AudioReader::new(music_dir, config.library.shuffle, db)?;

    let initial_playlist = audio_reader.build_playlist();

    if initial_playlist.is_empty() {
        return Err("No tracks found for initial playlist".into());
    }

    let listener_count = Arc::new(AtomicUsize::new(0));
    let director = Arc::new(Mutex::new(PlaybackDirector::new(initial_playlist)));

    {
        let director = director.clone();
        tokio::spawn(async move {
            loop {
                if let Ok(mut guard) = director.lock() {
                    guard.tick();
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        });
    }

    audio_reader.start_schedule_command_service(schedule_rx, director.clone());

    // Get timeline_rx for the server to use
    let timeline_rx = director
        .lock()
        .map_err(|e| format!("PlaybackDirector lock poisoned: {}", e))?
        .snapshot_tx
        .subscribe();

    // Create a processor for each enabled stream
    let mut stream_pipelines = Vec::new();

    for (name, stream_config) in &config.stream {
        if !stream_config.enabled {
            log::info!("Stream '{}' is disabled, skipping", name);
            continue;
        }

        log::info!(
            "Setting up stream '{}': {} @ {}kbps, {}Hz",
            name,
            stream_config.format,
            stream_config.bitrate,
            stream_config.sample_rate
        );

        let audio_processor = FFmpegProcessor::new(
            config.server.ffmpeg_path.clone(),
            stream_config.sample_rate,
            stream_config.bitrate,
            stream_config.channels,
            stream_config.format.clone(),
        );

        let listeners = listener_count.clone();
        let notify = Arc::new(Notify::new());
        let is_paused = Arc::new(AtomicBool::new(false));
        let buffer = FanoutBuffer::new(1000, 50 * 1024 * 1024).shared();

        audio_processor.check_ffmpeg_available()?;

        let timeline_rx_stream = director
            .lock()
            .map_err(|e| format!("PlaybackDirector lock poisoned: {}", e))?
            .snapshot_tx
            .subscribe();

        let (audio_tx, audio_rx) = unbounded();
        audio_processor.start_timeline_streaming_service(
            timeline_rx_stream,
            audio_tx,
            is_paused.clone(),
            listeners.clone(),
            notify.clone(),
        );

        stream_pipelines.push(StreamPipeline {
            name: name.clone(),
            buffer,
            receiver: audio_rx,
            bitrate: stream_config.bitrate,
            format: stream_config.format.clone(),
            listeners,
            notify,
            is_paused,
        });
    }

    if stream_pipelines.is_empty() {
        return Err("No enabled streams found in configuration".into());
    }

    log::info!("Initialized {} stream(s)", stream_pipelines.len());

    Ok((stream_pipelines, timeline_rx))
}

fn start_buffer_writer(
    stream_buffer: SharedFanoutBuffer,
    audio_rx: Receiver<AudioChunk>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match tokio::task::spawn_blocking({
                let audio_rx = audio_rx.clone();
                move || audio_rx.recv()
            })
            .await
            {
                Ok(Ok(audio_data)) => match stream_buffer.write() {
                    Ok(mut buffer) => buffer.push(audio_data.data),
                    Err(e) => {
                        log::error!("Failed to lock fanout buffer for write: {}", e);
                        break;
                    }
                },
                Ok(Err(e)) => {
                    log::error!("Failed to receive audio data: {}", e);
                    break;
                }
                Err(e) => {
                    log::error!("Task join error: {}", e);
                    break;
                }
            }
        }
    })
}

fn start_server(
    config: &Config,
    stream_buffers: Vec<StreamBufferEntry>,
    timeline_rx: watch::Receiver<TimelineSnapshot>,
) -> JoinHandle<()> {
    let server = IcecastServer::new(
        stream_buffers,
        config.station.name.clone(),
        config.station.description.clone(),
        config.station.genre.clone(),
        timeline_rx,
        config.server.public_url.clone(),
    );

    let public_url = config.server.public_url.clone();
    let port = config.server.port;
    tokio::spawn(async move {
        if let Err(e) = server.start_server(public_url, port).await {
            log::error!("Server failed: {}", e);
        }
    })
}

fn start_nightly_rescan(scanner: LibraryScanner) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let now = chrono::Local::now();

            // Calculate next scan time (3:00 AM tomorrow)
            let fallback_duration = std::time::Duration::from_secs(24 * 3600);

            let duration = match calculate_next_scan_duration(&now) {
                Ok(d) => d,
                Err(e) => {
                    log::warn!(
                        "Failed to calculate next scan time: {}, using 24h fallback",
                        e
                    );
                    fallback_duration
                }
            };

            let next_scan_info = now
                + chrono::Duration::from_std(duration)
                    .unwrap_or(chrono::Duration::seconds(24 * 3600));
            log::info!(
                "Next library scan scheduled at {}",
                next_scan_info.format("%Y-%m-%d %H:%M:%S")
            );

            tokio::time::sleep(duration).await;

            log::info!("Performing nightly library scan...");
            match scanner.incremental_scan() {
                Ok(result) => {
                    if result.added > 0 || result.updated > 0 || result.deleted > 0 {
                        log::info!(
                            "Nightly scan complete: +{} added, ~{} updated, -{} deleted",
                            result.added,
                            result.updated,
                            result.deleted
                        );
                    } else {
                        log::info!("Nightly scan complete: no changes detected");
                    }
                }
                Err(e) => log::error!("Nightly scan failed: {}", e),
            }
        }
    })
}

fn calculate_next_scan_duration(
    now: &chrono::DateTime<chrono::Local>,
) -> Result<std::time::Duration, String> {
    // Get tomorrow's date at 3:00 AM
    let tomorrow = now
        .date_naive()
        .succ_opt()
        .ok_or_else(|| "Failed to calculate next day".to_string())?;

    // Create time 3:00 AM - this is a constant so it's safe
    let time_3am = tomorrow
        .and_hms_opt(3, 0, 0)
        .expect("3:00:00 is always a valid time");

    // Handle DST ambiguity by using earliest()
    let next_scan = time_3am
        .and_local_timezone(chrono::Local)
        .earliest()
        .ok_or_else(|| "Failed to apply timezone".to_string())?;

    // Calculate duration; if negative (clock jump), fallback to 24h
    let duration = (next_scan - *now)
        .to_std()
        .map_err(|_| "Duration calculation resulted in negative value".to_string())?;

    Ok(duration)
}
fn log_startup_info(config: &Config) {
    if let Some(ref url) = config.server.public_url {
        log::info!("Starting Funkstrom server on {}", url);
    } else {
        log::info!(
            "Starting Funkstrom server on 0.0.0.0:{}",
            config.server.port
        );
    }
    log::info!("Music directory: {}", config.library.music_directory);
    log::info!("Station: {}", config.station.name);
}

fn log_server_urls(config: &Config) {
    log::info!("Funkstrom server started successfully!");

    let base_url = config.server.public_url.as_deref().unwrap_or("0.0.0.0");
    let port = config.server.port;

    // Log all enabled stream URLs
    for (name, stream_config) in &config.stream {
        if stream_config.enabled {
            if config.server.public_url.is_some() {
                log::info!(
                    "  Stream '{}': {}/{} ({}kbps)",
                    name,
                    base_url,
                    name,
                    stream_config.bitrate
                );
            } else {
                log::info!(
                    "  Stream '{}': {}:{}/{} ({}kbps)",
                    name,
                    base_url,
                    port,
                    name,
                    stream_config.bitrate
                );
            }
        }
    }

    if config.server.public_url.is_some() {
        log::info!("Status URL: {}/status", base_url);
        log::info!("Info URL: {}/", base_url);
    } else {
        log::info!("Status URL: {}:{}/status", base_url, port);
        log::info!("Info URL: {}:{}/", base_url, port);
    }
}
