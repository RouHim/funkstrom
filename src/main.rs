mod audio_buffer;
mod audio_metadata;
mod audio_processor;
mod audio_reader;
mod cli;
mod config;
mod library_db;
mod library_scanner;
mod m3u_parser;
mod schedule_engine;
mod server_icecast;
mod server_swagger;

use audio_buffer::StreamBuffer;
use audio_metadata::TrackMetadata;
use audio_processor::{AudioChunk, FFmpegProcessor};
use audio_reader::AudioReader;
use cli::get_config_path;
use config::Config;
use crossbeam_channel::Receiver;
use library_db::LibraryDatabase;
use library_scanner::LibraryScanner;
use schedule_engine::{PlaylistCommand, ScheduleEngine};
use server_icecast::IcecastServer;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

type AudioPipeline = (
    Receiver<PathBuf>,
    Receiver<AudioChunk>,
    Arc<Mutex<TrackMetadata>>,
);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    std::fs::create_dir_all("./data")?;

    // Load config
    let config_path = get_config_path();
    let config = Config::from_file(&config_path)?;

    log_startup_info(&config);

    // Initialize components
    let (db, scanner) = initialize_library(&config)?;
    let schedule_rx = setup_schedule_engine(&config);
    let (_track_rx, audio_rx, current_metadata) = setup_audio_pipeline(&config, db, schedule_rx)?;

    // Set up streaming buffer
    let stream_buffer = StreamBuffer::new(1000, 50 * 1024 * 1024);
    stream_buffer.start();

    // Start server
    let buffer_writer_handle = start_buffer_writer(&stream_buffer, audio_rx);
    let server_handle = start_server(&config, stream_buffer, current_metadata);

    log_server_urls(&config);

    // Start nightly rescan task
    let nightly_rescan_handle = start_nightly_rescan(scanner);

    // Wait for all tasks to complete
    tokio::select! {
        _ = server_handle => log::error!("Icecast server stopped"),
        _ = buffer_writer_handle => log::error!("Buffer writer stopped"),
        _ = nightly_rescan_handle => log::error!("Nightly rescan stopped"),
    }

    Ok(())
}

fn initialize_library(
    config: &Config,
) -> Result<(LibraryDatabase, LibraryScanner), Box<dyn std::error::Error>> {
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
) -> Result<AudioPipeline, Box<dyn std::error::Error>> {
    let music_dir = PathBuf::from(&config.library.music_directory);
    let audio_reader =
        AudioReader::new(music_dir, config.library.shuffle, config.library.repeat, db)?;

    let audio_processor = FFmpegProcessor::new(
        config.server.ffmpeg_path.clone(),
        config.stream.sample_rate,
        config.stream.bitrate,
        config.stream.channels,
        config.stream.format.clone(),
    );

    audio_processor.check_ffmpeg_available()?;

    let current_metadata = audio_reader.get_current_metadata();
    let track_rx = audio_reader.start_playlist_service(schedule_rx);
    let audio_rx = audio_processor.start_streaming_service(track_rx.clone());

    Ok((track_rx, audio_rx, current_metadata))
}

fn start_buffer_writer(
    stream_buffer: &StreamBuffer,
    audio_rx: Receiver<AudioChunk>,
) -> JoinHandle<()> {
    let buffer_input_tx = stream_buffer.get_input_sender();

    tokio::spawn(async move {
        loop {
            match tokio::task::spawn_blocking({
                let audio_rx = audio_rx.clone();
                move || audio_rx.recv()
            })
            .await
            {
                Ok(Ok(audio_data)) => {
                    if let Err(e) = buffer_input_tx.send(audio_data.data) {
                        log::error!("Failed to send audio data to buffer: {}", e);
                        break;
                    }
                }
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
    stream_buffer: StreamBuffer,
    current_metadata: Arc<Mutex<TrackMetadata>>,
) -> JoinHandle<()> {
    let server = IcecastServer::new(
        stream_buffer,
        config.station.station_name.clone(),
        config.station.description.clone(),
        config.station.genre.clone(),
        config.stream.bitrate,
        current_metadata,
    );

    let bind_address = config.server.bind_address.clone();
    let port = config.server.port;
    tokio::spawn(async move {
        server.start_server(&bind_address, port).await;
    })
}

fn start_nightly_rescan(scanner: LibraryScanner) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let now = chrono::Local::now();
            let next_scan = now
                .date_naive()
                .succ_opt()
                .unwrap()
                .and_hms_opt(3, 0, 0)
                .unwrap()
                .and_local_timezone(chrono::Local)
                .unwrap();
            let duration = (next_scan - now).to_std().unwrap();

            log::info!(
                "Next library scan scheduled at {}",
                next_scan.format("%Y-%m-%d %H:%M:%S")
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

fn log_startup_info(config: &Config) {
    log::info!(
        "Starting Funkstrom server on {}:{}",
        config.server.bind_address,
        config.server.port
    );
    log::info!("Music directory: {}", config.library.music_directory);
    log::info!("Station: {}", config.station.station_name);
}

fn log_server_urls(config: &Config) {
    log::info!("Funkstrom server started successfully!");
    log::info!(
        "Stream URL: http://{}:{}/stream",
        config.server.bind_address,
        config.server.port
    );
    log::info!(
        "Status URL: http://{}:{}/status",
        config.server.bind_address,
        config.server.port
    );
    log::info!(
        "Info URL: http://{}:{}/",
        config.server.bind_address,
        config.server.port
    );
}
