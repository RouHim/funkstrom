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
mod server_metadata;
mod server_swagger;

use audio_buffer::StreamBuffer;
use audio_processor::FFmpegProcessor;
use audio_reader::AudioReader;
use cli::get_config_path;
use config::Config;
use library_db::LibraryDatabase;
use library_scanner::LibraryScanner;
use schedule_engine::ScheduleEngine;
use server_icecast::IcecastServer;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    std::fs::create_dir_all("./data")?;

    let config_path = get_config_path();
    let config = Config::from_file(&config_path)?;

    log::info!(
        "Starting iRadio server on {}:{}",
        config.server.bind_address,
        config.server.port
    );
    log::info!("Music directory: {}", config.library.music_directory);
    log::info!("Station: {}", config.station.station_name);

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

    let schedule_command_rx = if let Some(ref schedule_config) = config.schedule {
        if !schedule_config.programs.is_empty() && schedule_config.programs.iter().any(|p| p.active)
        {
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
        } else {
            log::info!("No active programs found, running in library-only mode");
            None
        }
    } else {
        log::info!("No schedule configuration found, running in library-only mode");
        None
    };

    let audio_reader =
        AudioReader::new(music_dir, config.library.shuffle, config.library.repeat, db)?;

    // FFmpegProcessor::new needs 4 parameters: ffmpeg_path, sample_rate, bitrate, channels
    let audio_processor = FFmpegProcessor::new(
        config.server.ffmpeg_path.clone(),
        config.stream.sample_rate,
        config.stream.bitrate,
        config.stream.channels,
    );

    // Check FFmpeg availability
    audio_processor.check_ffmpeg_available()?;

    // Create stream buffer
    let stream_buffer = StreamBuffer::new(
        1000,             // max chunks
        50 * 1024 * 1024, // 50MB max bytes
    );

    // Start services using correct service architecture pattern

    // Get metadata reference before audio_reader is consumed
    let current_metadata = audio_reader.get_current_metadata();

    let track_rx = audio_reader.start_playlist_service(schedule_command_rx);

    // 2. Start FFmpeg processor service (consumes self, needs track_rx input)
    let audio_rx = audio_processor.start_streaming_service(track_rx);

    // 3. Start buffer service (returns (), not JoinHandle)
    stream_buffer.start();

    // 4. Get buffer input sender and connect audio processor output
    let buffer_input_tx = stream_buffer.get_input_sender();
    let buffer_writer_handle = tokio::spawn(async move {
        loop {
            // Use spawn_blocking for crossbeam channel recv
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
    });

    // 5. Start Icecast server (async method, returns ())
    let server = IcecastServer::new(
        stream_buffer,
        config.station.station_name.clone(),
        config.station.description.clone(),
        config.station.genre.clone(),
        config.stream.bitrate,
        current_metadata,
    );

    let server_handle = tokio::spawn(async move {
        server.start_server(config.server.port).await;
    });

    log::info!("iRadio server started successfully!");
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

    let nightly_rescan_handle = tokio::spawn(async move {
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
    });

    // Wait for all tasks to complete (they should run indefinitely)
    tokio::select! {
        _ = server_handle => log::error!("Icecast server stopped"),
        _ = buffer_writer_handle => log::error!("Buffer writer stopped"),
        _ = nightly_rescan_handle => log::error!("Nightly rescan stopped"),
    }

    Ok(())
}
