mod audio_buffer;
mod audio_metadata;
mod audio_processor;
mod audio_reader;
mod cli;
mod config;
mod server_icecast;
mod server_metadata;
mod server_swagger;

use audio_buffer::StreamBuffer;
use audio_processor::FFmpegProcessor;
use audio_reader::AudioReader;
use cli::get_config_path;
use config::Config;
use server_icecast::IcecastServer;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let config_path = get_config_path();
    let config = Config::from_file(&config_path)?;

    log::info!(
        "Starting iRadio server on {}:{}",
        config.server.bind_address,
        config.server.port
    );
    log::info!("Music directory: {}", config.library.music_directory);
    log::info!("Station: {}", config.stream.station_name);

    // Initialize components with correct API signatures
    let music_dir = PathBuf::from(&config.library.music_directory);
    let audio_reader = AudioReader::new(music_dir, config.library.shuffle, config.library.repeat)?;

    // FFmpegProcessor::new needs 4 parameters: ffmpeg_path, sample_rate, bitrate, channels
    let audio_processor = FFmpegProcessor::new(
        config.ffmpeg.path.clone(),
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

    // 1. Start AudioReader playlist service (consumes self, returns channels)
    let track_rx = audio_reader.start_playlist_service();

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
        config.stream.station_name.clone(),
        config.stream.description.clone(),
        config.stream.genre.clone(),
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

    // Wait for all tasks to complete (they should run indefinitely)
    tokio::select! {
        _ = server_handle => log::error!("Icecast server stopped"),
        _ = buffer_writer_handle => log::error!("Buffer writer stopped"),
    }

    Ok(())
}
