use crate::fanout_buffer::SharedFanoutBuffer;
use crate::playback_director::TimelineSnapshot;
use crate::server_swagger;
use bytes::Bytes;
use minijinja::Environment;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::UnboundedReceiverStream;
use warp::{http::HeaderMap, Filter, Reply};

// JSON response structures for serialization
#[derive(Serialize)]
struct StatusResponse {
    station_name: String,
    station_description: String,
    station_genre: String,
    streams: Vec<StreamStatus>,
    uptime: String,
}

#[derive(Serialize)]
struct StreamStatus {
    name: String,
    bitrate: u32,
    status: String,
    buffer_chunks: usize,
    buffer_bytes: usize,
    listeners: usize,
}

// Template context structures
#[derive(Serialize)]
struct InfoPageContext {
    station_name: String,
    current_track: String,
    album: String,
    station_description: String,
    station_genre: String,
    bitrate: u32,
    bind_address: String,
    port: u16,
    streams: Vec<StreamLink>,
    first_stream: String,
}

#[derive(Serialize)]
struct StreamLink {
    name: String,
    bitrate: u32,
    url: String,
}

// RAII guard for listener tracking
struct ListenerGuard {
    listeners: Arc<AtomicUsize>,
    #[allow(dead_code)]
    notify: Arc<tokio::sync::Notify>,
}

impl ListenerGuard {
    fn new(listeners: Arc<AtomicUsize>, notify: Arc<tokio::sync::Notify>) -> Self {
        let was_zero = listeners.fetch_add(1, Ordering::SeqCst) == 0;
        if was_zero {
            notify.notify_one();
        }
        Self { listeners, notify }
    }
}

impl Drop for ListenerGuard {
    fn drop(&mut self) {
        self.listeners.fetch_sub(1, Ordering::SeqCst);
    }
}

// Context for handling stream requests
#[derive(Clone)]
struct StreamContext {
    buffer: SharedFanoutBuffer,
    bitrate: u32,
    format: String,
    station_name: String,
    station_description: String,
    station_genre: String,
    listeners: Arc<AtomicUsize>,
    notify: Arc<tokio::sync::Notify>,
    timeline_rx: watch::Receiver<TimelineSnapshot>,
}

#[derive(Clone)]
pub struct IcecastServer {
    streams: Arc<Vec<StreamEndpoint>>,
    station_name: String,
    station_description: String,
    station_genre: String,
    timeline_rx: watch::Receiver<TimelineSnapshot>,
    bind_address: Arc<Mutex<String>>,
    port: Arc<Mutex<u16>>,
    start_time: Instant,
}

#[derive(Clone)]
struct StreamEndpoint {
    name: String,
    buffer: SharedFanoutBuffer,
    bitrate: u32,
    format: String,
    listeners: Arc<AtomicUsize>,
    notify: Arc<tokio::sync::Notify>,
    is_paused: Arc<AtomicBool>,
}

pub type StreamBufferEntry = (
    String,
    SharedFanoutBuffer,
    u32,
    String,
    Arc<AtomicUsize>,
    Arc<tokio::sync::Notify>,
    Arc<AtomicBool>,
);

/// Build an ICY metadata block per the Icecast/SHOUTcast protocol.
///
/// Format: 1 byte block count N, followed by N×16 bytes of null-padded UTF-8.
/// N = ceil(strlen / 16), clamped to 255 max.
fn build_icy_metadata_block(artist_title: &str) -> Vec<u8> {
    let stream_title = format!("StreamTitle='{}';StreamUrl='';", artist_title);
    let len = stream_title.len();
    let blocks = len.div_ceil(16).min(255) as u8;
    let padded_len = blocks as usize * 16;
    let copy_len = len.min(padded_len);
    let mut block = vec![0u8; 1 + padded_len];
    block[0] = blocks;
    block[1..1 + copy_len].copy_from_slice(&stream_title.as_bytes()[..copy_len]);
    block
}

/// Process a chunk of audio data, splitting at `metaint` boundaries and inserting
/// ICY metadata blocks. Returns output blocks that are sent to the client in order.
///
/// Maintains `bytes_since_meta` and `last_meta_str` across calls so metadata
/// boundaries stay aligned and change detection works across chunks.
fn process_audio_with_icy(
    chunk: Bytes,
    metaint: usize,
    bytes_since_meta: &mut usize,
    last_meta_str: &mut String,
    current_meta_str: &str,
) -> Vec<Bytes> {
    let mut output = Vec::new();
    let mut offset = 0;
    while offset < chunk.len() {
        let remaining_in_interval = metaint - *bytes_since_meta;
        let consume = remaining_in_interval.min(chunk.len() - offset);

        if consume > 0 {
            output.push(chunk.slice(offset..offset + consume));
            offset += consume;
            *bytes_since_meta += consume;
        }

        if *bytes_since_meta >= metaint {
            let block: Vec<u8> = if *current_meta_str != *last_meta_str {
                *last_meta_str = current_meta_str.to_string();
                build_icy_metadata_block(last_meta_str)
            } else {
                vec![0x00]
            };
            output.push(Bytes::from(block));
            *bytes_since_meta = 0;
        }
    }
    output
}

impl IcecastServer {
    pub fn new(
        stream_buffers: Vec<StreamBufferEntry>,
        station_name: String,
        station_description: String,
        station_genre: String,
        timeline_rx: watch::Receiver<TimelineSnapshot>,
    ) -> Self {
        let streams = stream_buffers
            .into_iter()
            .map(
                |(name, buffer, bitrate, format, listeners, notify, is_paused)| StreamEndpoint {
                    name,
                    buffer,
                    bitrate,
                    format,
                    listeners,
                    notify,
                    is_paused,
                },
            )
            .collect();

        Self {
            streams: Arc::new(streams),
            station_name,
            station_description,
            station_genre,
            timeline_rx,
            bind_address: Arc::new(Mutex::new(String::new())),
            port: Arc::new(Mutex::new(0)),
            start_time: Instant::now(),
        }
    }

    pub async fn start_server(
        &self,
        bind_address: &str,
        port: u16,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Store bind_address and port for use in info page
        *self.bind_address.lock().unwrap_or_else(|e| e.into_inner()) = bind_address.to_string();
        *self.port.lock().unwrap_or_else(|e| e.into_inner()) = port;

        let server = Arc::new(self.clone());

        // Dynamic stream route handler
        let streams_map = self.streams.clone();
        let station_name = self.station_name.clone();
        let station_description = self.station_description.clone();
        let station_genre = self.station_genre.clone();
        let timeline_rx = self.timeline_rx.clone();

        let stream_route = warp::path::param::<String>()
            .and(warp::get())
            .and(warp::header::headers_cloned())
            .and_then(move |stream_name: String, headers: HeaderMap| {
                let streams = streams_map.clone();
                let station_name = station_name.clone();
                let station_description = station_description.clone();
                let station_genre = station_genre.clone();
                let timeline_rx = timeline_rx.clone();

                async move {
                    // Find the stream by name and create context
                    for stream in streams.iter() {
                        if stream.name == stream_name {
                            let context = StreamContext {
                                buffer: stream.buffer.clone(),
                                bitrate: stream.bitrate,
                                format: stream.format.clone(),
                                station_name: station_name.clone(),
                                station_description: station_description.clone(),
                                station_genre: station_genre.clone(),
                                listeners: stream.listeners.clone(),
                                notify: stream.notify.clone(),
                                timeline_rx: timeline_rx.clone(),
                            };
                            return Self::handle_stream_request(headers, context).await;
                        }
                    }
                    Err(warp::reject::not_found())
                }
            });

        let status_route = warp::path("status").and(warp::get()).and_then({
            let server = Arc::clone(&server);
            move || {
                let server = Arc::clone(&server);
                async move { server.handle_status_request().await }
            }
        });

        let info_route = warp::path::end().and(warp::get()).and_then({
            let server = Arc::clone(&server);
            move || {
                let server = Arc::clone(&server);
                async move { server.handle_info_request().await }
            }
        });

        let current_route = warp::path("current").and(warp::get()).and_then({
            let server = Arc::clone(&server);
            move || {
                let server = Arc::clone(&server);
                async move { server.handle_current_request().await }
            }
        });

        // Swagger API documentation routes
        let swagger_ui_route = server_swagger::swagger_ui();
        let openapi_spec_route = server_swagger::openapi_spec();

        let routes = stream_route
            .or(status_route)
            .or(current_route)
            .or(swagger_ui_route)
            .or(openapi_spec_route)
            .or(info_route);

        log::info!("Starting Funkstrom server on {}:{}", bind_address, port);
        log::info!("API Docs: http://{}:{}/api-docs", bind_address, port);

        let addr: std::net::SocketAddr = format!("{}:{}", bind_address, port).parse()?;
        warp::serve(routes).run(addr).await;
        Ok(())
    }

    async fn handle_stream_request(
        headers: HeaderMap,
        context: StreamContext,
    ) -> Result<impl Reply, warp::Rejection> {
        log::info!("New client connected for streaming");

        let user_agent = headers
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("Unknown");

        log::info!("Client User-Agent: {}", user_agent);

        // Check for Range header - we don't support seeking in live streams
        if headers.contains_key("range") {
            log::warn!("Client attempted to seek on live stream, ignoring Range header");
        }

        // Detect ICY metadata support
        let icy_metadata_requested = headers
            .get("Icy-MetaData")
            .or_else(|| headers.get("icy-metadata"))
            .and_then(|v| v.to_str().ok())
            .map(|v| v.trim() == "1")
            .unwrap_or(false);

        let format_lower = context.format.to_lowercase();
        let icy_metadata_supported = matches!(format_lower.as_str(), "mp3" | "aac");
        let do_icy_metadata = icy_metadata_requested && icy_metadata_supported;

        if icy_metadata_requested && !icy_metadata_supported {
            log::debug!(
                "Client requested ICY metadata but format '{}' does not support it",
                context.format
            );
        }

        let (tx, rx) = mpsc::unbounded_channel();
        let buffer = context.buffer.clone();
        let listeners = context.listeners.clone();
        let notify = context.notify.clone();
        let timeline_rx = context.timeline_rx.clone();
        let do_icy_metadata_clone = do_icy_metadata;

        tokio::spawn(async move {
            let _guard = ListenerGuard::new(listeners, notify);
            let mut cursor = match buffer.write() {
                Ok(guard) => guard.new_cursor_with_burst(50),
                Err(e) => {
                    log::error!("Failed to initialize listener cursor: {}", e);
                    return;
                }
            };
            let mut last_data_time = Instant::now();
            let timeout_duration = Duration::from_secs(30);

            const METADATA_INTERVAL: usize = 16000;
            // byte counters start at 0 from first audio byte; no initial metadata block
            // (VLC, GStreamer icydemux count from HTTP body start — a leading block
            //  would be treated as audio, permanently misaligning all metadata boundaries)
            let mut bytes_since_meta: usize = 0;
            let mut last_meta_str = String::new();

            loop {
                let chunk = match buffer.read() {
                    Ok(guard) => guard.read_from_cursor(&mut cursor),
                    Err(e) => {
                        log::error!("Failed to lock fanout buffer for read: {}", e);
                        break;
                    }
                };

                if let Some(chunk) = chunk {
                    if do_icy_metadata_clone {
                        let current_meta = timeline_rx.borrow().current_metadata.to_icy_metadata();
                        for block in process_audio_with_icy(
                            chunk,
                            METADATA_INTERVAL,
                            &mut bytes_since_meta,
                            &mut last_meta_str,
                            &current_meta,
                        ) {
                            if tx.send(Ok(block)).is_err() {
                                log::info!("Client disconnected");
                                return;
                            }
                        }
                    } else {
                        if tx.send(Ok::<_, warp::Error>(chunk)).is_err() {
                            log::info!("Client disconnected");
                            break;
                        }
                    }
                    last_data_time = Instant::now();
                } else {
                    if last_data_time.elapsed() > timeout_duration {
                        log::warn!("No data available for too long, disconnecting client");
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        });

        let stream = UnboundedReceiverStream::new(rx);

        let server_version = format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));

        let mut response_builder = warp::http::Response::builder()
            .header(
                "Content-Type",
                match format_lower.as_str() {
                    "aac" => "audio/aac",
                    "ogg" => "audio/ogg",
                    "opus" => "audio/ogg",
                    _ => "audio/mpeg",
                },
            )
            .header("Cache-Control", "no-cache, no-store")
            .header("Connection", "close")
            .header("Pragma", "no-cache")
            .header("Accept-Ranges", "none")
            .header("Access-Control-Allow-Origin", "*")
            .header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
            .header("Access-Control-Allow-Headers", "Content-Type")
            .header("icy-name", &context.station_name)
            .header("icy-description", &context.station_description)
            .header("icy-genre", &context.station_genre)
            .header("icy-br", context.bitrate.to_string())
            .header("Server", &server_version);

        if do_icy_metadata {
            response_builder = response_builder.header("icy-metaint", "16000");
        }

        let response = response_builder
            .body(hyper::Body::wrap_stream(stream))
            .map_err(|e| {
                log::error!("Failed to build HTTP response: {}", e);
                warp::reject::reject()
            })?;

        Ok(response)
    }

    async fn handle_status_request(&self) -> Result<impl Reply, warp::Rejection> {
        let streams = self
            .streams
            .iter()
            .map(|stream| {
                let (chunks, bytes, is_running) = match stream.buffer.read() {
                    Ok(guard) => (guard.ring.len(), guard.current_bytes, true),
                    Err(e) => {
                        log::error!("Failed to read buffer status: {}", e);
                        (0, 0, false)
                    }
                };
                let is_paused = stream.is_paused.load(Ordering::SeqCst);
                let listeners = stream.listeners.load(Ordering::SeqCst);

                let status = if is_running && is_paused {
                    "idle".to_string()
                } else if is_running {
                    "online".to_string()
                } else {
                    "offline".to_string()
                };

                StreamStatus {
                    name: stream.name.clone(),
                    bitrate: stream.bitrate,
                    status,
                    buffer_chunks: chunks,
                    buffer_bytes: bytes,
                    listeners,
                }
            })
            .collect();

        let response = StatusResponse {
            station_name: self.station_name.clone(),
            station_description: self.station_description.clone(),
            station_genre: self.station_genre.clone(),
            streams,
            uptime: {
                let elapsed = self.start_time.elapsed().as_secs();
                let h = elapsed / 3600;
                let m = (elapsed % 3600) / 60;
                let s = elapsed % 60;
                format!("{}h {}m {}s", h, m, s)
            },
        };

        let json = serde_json::to_string(&response).map_err(|e| {
            log::error!("Failed to serialize status response: {}", e);
            warp::reject::reject()
        })?;

        Ok(warp::reply::with_header(
            json,
            "Content-Type",
            "application/json",
        ))
    }

    async fn handle_current_request(&self) -> Result<impl Reply, warp::Rejection> {
        let snapshot = self.timeline_rx.borrow().clone();
        let json = snapshot.current_metadata.to_json();

        Ok(warp::reply::with_header(
            json,
            "Content-Type",
            "application/json",
        ))
    }

    async fn handle_info_request(&self) -> Result<impl Reply, warp::Rejection> {
        let snapshot = self.timeline_rx.borrow().clone();
        let current_track = snapshot.current_metadata.to_icy_metadata();
        let album = &snapshot.current_metadata.album;

        let bind_address = self
            .bind_address
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let port = *self.port.lock().unwrap_or_else(|e| e.into_inner());

        // Build streams list for template context
        let streams: Vec<StreamLink> = self
            .streams
            .iter()
            .map(|stream| StreamLink {
                name: stream.name.clone(),
                bitrate: stream.bitrate,
                url: format!("http://{}:{}/{}", bind_address, port, stream.name),
            })
            .collect();

        // Use the first stream for the audio player
        let first_stream = self
            .streams
            .first()
            .map(|s| s.name.clone())
            .unwrap_or_else(|| "stream".to_string());
        let first_bitrate = self.streams.first().map(|s| s.bitrate).unwrap_or(128);

        let context = InfoPageContext {
            station_name: self.station_name.clone(),
            current_track,
            album: album.clone(),
            station_description: self.station_description.clone(),
            station_genre: self.station_genre.clone(),
            bitrate: first_bitrate,
            bind_address: bind_address.clone(),
            port,
            streams,
            first_stream,
        };

        const TEMPLATE_STR: &str = include_str!("../templates/info.html");

        let mut env = Environment::new();
        env.add_template("info", TEMPLATE_STR).map_err(|e| {
            log::error!("Template error: {}", e);
            warp::reject::reject()
        })?;

        let tmpl = env.get_template("info").map_err(|e| {
            log::error!("Template get error: {}", e);
            warp::reject::reject()
        })?;

        let rendered = tmpl.render(&context).map_err(|e| {
            log::error!("Render error: {}", e);
            warp::reject::reject()
        })?;

        Ok(warp::reply::with_header(
            rendered,
            "Content-Type",
            "text/html; charset=utf-8",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn given_new_guard_when_created_then_increments_counter() {
        let listeners = Arc::new(AtomicUsize::new(0));
        let notify = Arc::new(tokio::sync::Notify::new());

        assert_eq!(listeners.load(Ordering::SeqCst), 0);
        let _guard = ListenerGuard::new(listeners.clone(), notify);
        assert_eq!(listeners.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn given_guard_when_dropped_then_decrements_counter() {
        let listeners = Arc::new(AtomicUsize::new(1));
        let notify = Arc::new(tokio::sync::Notify::new());

        let guard = ListenerGuard::new(listeners.clone(), notify);
        assert_eq!(listeners.load(Ordering::SeqCst), 2);
        drop(guard);
        assert_eq!(listeners.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn given_multiple_guards_when_one_dropped_then_counter_reflects_remaining() {
        let listeners = Arc::new(AtomicUsize::new(0));
        let notify = Arc::new(tokio::sync::Notify::new());

        let guard1 = ListenerGuard::new(listeners.clone(), notify.clone());
        let guard2 = ListenerGuard::new(listeners.clone(), notify.clone());
        let guard3 = ListenerGuard::new(listeners.clone(), notify.clone());

        assert_eq!(listeners.load(Ordering::SeqCst), 3);
        drop(guard1);
        assert_eq!(listeners.load(Ordering::SeqCst), 2);
        drop(guard2);
        assert_eq!(listeners.load(Ordering::SeqCst), 1);
        drop(guard3);
        assert_eq!(listeners.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn given_stream_status_with_listeners_when_serialized_then_json_contains_listeners() {
        let status = StreamStatus {
            name: "test_stream".to_string(),
            bitrate: 128,
            status: "online".to_string(),
            buffer_chunks: 10,
            buffer_bytes: 4096,
            listeners: 5,
        };

        let json = serde_json::to_string(&status).expect("serialization failed");
        assert!(json.contains("\"listeners\":5"));
    }

    // --- build_icy_metadata_block tests ---

    #[test]
    fn given_short_title_when_building_metadata_block_then_pads_to_16_bytes() {
        // "Artist - Title" = 15 chars
        // "StreamTitle='Artist - Title';StreamUrl='';" = 13 + 15 + 18 = 46 chars
        // N = ceil(46 / 16) = 3, padded = 48 bytes
        let block = build_icy_metadata_block("Artist - Title");
        assert_eq!(block.len(), 1 + 3 * 16); // 49 bytes
        assert_eq!(block[0], 3); // 3 blocks
        let expected_prefix = b"StreamTitle='Artist - Title';StreamUrl='';";
        assert_eq!(&block[1..1 + expected_prefix.len()], expected_prefix);
        // Remaining bytes should be null padding
        assert!(block[1 + expected_prefix.len()..].iter().all(|&b| b == 0));
    }

    #[test]
    fn given_exact_16_boundary_when_building_metadata_block_then_correct_block_count() {
        // 31-byte payload → ceil(31/16) = 2 blocks
        let block = build_icy_metadata_block("AB");
        assert_eq!(block[0], 2);
        assert_eq!(block.len(), 1 + 2 * 16);
    }

    #[test]
    fn given_empty_title_when_building_metadata_block_then_still_valid() {
        let block = build_icy_metadata_block("");
        assert_eq!(block[0], 2); // "StreamTitle='';StreamUrl='';" = 30 chars → ceil(30/16)=2
        assert!(block[1..].iter().any(|&b| b != 0)); // has content
    }

    #[test]
    fn given_very_long_title_when_building_metadata_block_then_clamped_to_255_blocks() {
        let long = "X".repeat(5000);
        let block = build_icy_metadata_block(&long);
        assert_eq!(block[0], 255); // clamped
        assert_eq!(block.len(), 1 + 255 * 16); // 4081 bytes
    }

    #[test]
    fn given_special_characters_when_building_metadata_block_then_utf8_preserved() {
        let block = build_icy_metadata_block("Motörhead - Åce of Spädes");
        let content = String::from_utf8_lossy(&block[1..]);
        assert!(content.contains("Motörhead"));
        assert!(content.contains("Åce of Spädes"));
    }
}

// --- process_audio_with_icy tests ---

#[test]
fn given_bytes_since_meta_zero_when_short_chunk_then_all_output_is_audio() {
    // Regression: no metadata block must appear at byte 0 of the stream.
    let chunk = Bytes::from(vec![0xAA; 100]);
    let mut bytes_since_meta = 0;
    let mut last_meta_str = String::new();

    let blocks = process_audio_with_icy(
        chunk,
        16000,
        &mut bytes_since_meta,
        &mut last_meta_str,
        "Some Artist - Some Track",
    );

    // All blocks are audio — no metadata injected
    assert!(!blocks.is_empty());
    for block in &blocks {
        assert!(
            block.len() > 1 || !block.is_empty(),
            "all blocks under metaint should be audio chunks"
        );
    }
    // Only one audio chunk (no splitting needed)
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].len(), 100);
    assert_eq!(bytes_since_meta, 100);
    assert_eq!(last_meta_str, ""); // unchanged — no boundary hit
}

#[test]
fn given_bytes_near_metaint_when_chunk_crosses_boundary_then_metadata_injected() {
    // bytes_since_meta at 15900, chunk of 200 bytes.
    // Should produce: 100 bytes audio, metadata block, 100 bytes audio.
    let chunk = Bytes::from(vec![0xBB; 200]);
    let mut bytes_since_meta = 15900;
    let mut last_meta_str = String::new();

    let blocks = process_audio_with_icy(
        chunk,
        16000,
        &mut bytes_since_meta,
        &mut last_meta_str,
        "Artist - Title",
    );

    assert_eq!(blocks.len(), 3, "audio, metadata, audio");
    // First block: audio, 100 bytes (fills remaining interval)
    assert_eq!(blocks[0].len(), 100);
    // Second block: metadata block
    assert!(
        blocks[1].len() > 1,
        "metadata block must be more than empty \\x00"
    );
    // Third block: remaining 100 bytes of audio
    assert_eq!(blocks[2].len(), 100);
    // Counter must reset after metadata boundary
    assert_eq!(bytes_since_meta, 100);
    // last_meta_str must be updated
    assert_eq!(last_meta_str, "Artist - Title");
}

#[test]
fn given_metadata_unchanged_when_boundary_hit_then_empty_block() {
    let chunk = Bytes::from(vec![0xCC; 16000]);
    let mut bytes_since_meta = 15900;
    let mut last_meta_str = String::from("Artist - Title");

    let blocks = process_audio_with_icy(
        chunk,
        16000,
        &mut bytes_since_meta,
        &mut last_meta_str,
        "Artist - Title", // same as last_meta_str
    );

    assert_eq!(blocks.len(), 3);
    // Metadata block should be single zero byte
    assert_eq!(blocks[1].len(), 1);
    assert_eq!(blocks[1][0], 0x00);
    // last_meta_str unchanged
    assert_eq!(last_meta_str, "Artist - Title");
}

#[test]
fn given_metadata_changed_when_boundary_hit_then_full_block_and_counter_update() {
    let chunk = Bytes::from(vec![0xDD; 16000]);
    let mut bytes_since_meta = 15900;
    let mut last_meta_str = String::from("Old Artist - Old Title");

    let blocks = process_audio_with_icy(
        chunk,
        16000,
        &mut bytes_since_meta,
        &mut last_meta_str,
        "New Artist - New Title",
    );

    assert_eq!(blocks.len(), 3);
    // Metadata block must be > 1 byte (not empty)
    assert!(blocks[1].len() > 1);
    // last_meta_str updates to new value
    assert_eq!(last_meta_str, "New Artist - New Title");
}

#[test]
fn given_chunk_spanning_multiple_boundaries_then_metadata_at_each() {
    // 50000 bytes, metaint=16000 → boundaries at 16000, 32000, 48000
    // Metadata unchanged → all empty blocks
    let chunk = Bytes::from(vec![0xEE; 50000]);
    let mut bytes_since_meta = 0;
    let mut last_meta_str = String::from("Track A - Song A");

    let blocks = process_audio_with_icy(
        chunk,
        16000,
        &mut bytes_since_meta,
        &mut last_meta_str,
        "Track A - Song A", // unchanged
    );

    // Layout: A=[16000] M=[empty] A=[16000] M=[empty] A=[16000] M=[empty] A=[2000]
    assert_eq!(blocks.len(), 7);
    assert_eq!(blocks[0].len(), 16000);
    assert_eq!(blocks[1].len(), 1);
    assert_eq!(blocks[1][0], 0x00);
    assert_eq!(blocks[2].len(), 16000);
    assert_eq!(blocks[3].len(), 1);
    assert_eq!(blocks[3][0], 0x00);
    assert_eq!(blocks[4].len(), 16000);
    assert_eq!(blocks[5].len(), 1);
    assert_eq!(blocks[5][0], 0x00);
    assert_eq!(blocks[6].len(), 2000);
    assert_eq!(bytes_since_meta, 2000);
    assert_eq!(last_meta_str, "Track A - Song A");
}

#[test]
fn given_chunk_exactly_at_metaint_boundary_then_only_metadata_emitted() {
    // bytes_since_meta already at metaint, so chunk should trigger metadata right away
    let chunk = Bytes::from(vec![0xFF; 100]);
    let mut bytes_since_meta = 16000;
    let mut last_meta_str = String::new();

    let blocks = process_audio_with_icy(
        chunk,
        16000,
        &mut bytes_since_meta,
        &mut last_meta_str,
        "Artist - Track",
    );

    // First: metadata (boundary already reached), then 100 bytes audio
    assert_eq!(blocks.len(), 2);
    assert!(blocks[0].len() > 1, "first block is metadata");
    assert_eq!(blocks[1].len(), 100, "second block is audio");
    assert_eq!(bytes_since_meta, 100);
}

#[test]
fn given_empty_chunk_when_processing_then_no_output() {
    let chunk = Bytes::new();
    let mut bytes_since_meta = 0;
    let mut last_meta_str = String::new();

    let blocks = process_audio_with_icy(
        chunk,
        16000,
        &mut bytes_since_meta,
        &mut last_meta_str,
        "Artist - Track",
    );

    assert!(blocks.is_empty());
    assert_eq!(bytes_since_meta, 0);
    assert_eq!(last_meta_str, "");
}
