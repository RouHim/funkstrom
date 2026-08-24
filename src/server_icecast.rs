use crate::fanout_buffer::SharedFanoutBuffer;
use crate::playback_director::TimelineSnapshot;
use crate::server_swagger;
use bytes::Bytes;

use futures_core::Stream;
use serde::Serialize;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::{mpsc, watch};
use warp::{http::HeaderMap, Filter, Reply};

// JSON response structures for serialization
const VERSION: &str = include_str!(concat!(env!("OUT_DIR"), "/version.txt"));
#[derive(Serialize)]
struct StatusResponse {
    station_name: String,
    station_description: String,
    station_genre: String,
    streams: Vec<StreamStatus>,
    uptime: String,
    version: String,
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
struct InfoPageContext {
    station_name: String,
    current_track: String,
    album: String,
    station_description: String,
    station_genre: String,
    bitrate: u32,
    public_url: String,
    streams: Vec<StreamLink>,
    first_stream: String,
    cover_url: String,
    version: String,
    listeners: usize,
    uptime: String,
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
    _notify: Arc<tokio::sync::Notify>,
}

impl ListenerGuard {
    fn new(listeners: Arc<AtomicUsize>, notify: Arc<tokio::sync::Notify>) -> Self {
        let was_zero = listeners.fetch_add(1, Ordering::SeqCst) == 0;
        if was_zero {
            notify.notify_one();
        }
        Self {
            listeners,
            _notify: notify,
        }
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
    cover_url: Option<String>,
}

#[derive(Clone)]
pub struct IcecastServer {
    streams: Arc<Vec<StreamEndpoint>>,
    station_name: String,
    station_description: String,
    station_genre: String,
    timeline_rx: watch::Receiver<TimelineSnapshot>,
    public_url: Option<String>,
    port: u16,
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

/// Extract cover art from an audio file's embedded tags.
/// Returns (image_bytes, mime_type) if the file has embedded cover art.
fn extract_cover_from_file(path: &std::path::Path) -> Option<(Vec<u8>, String)> {
    let tag = audiotags::Tag::new().read_from_path(path).ok()?;
    let album = tag.album()?;
    let cover = album.cover?;
    let mime: &'static str = cover.mime_type.into();
    Some((cover.data.to_vec(), mime.to_string()))
}
fn build_icy_metadata_block(artist_title: &str, stream_url: Option<&str>) -> Vec<u8> {
    let stream_url = stream_url.unwrap_or("");
    let stream_title = format!("StreamTitle='{}';StreamUrl='{}';", artist_title, stream_url);
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
    stream_url: Option<&str>,
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
                build_icy_metadata_block(last_meta_str, stream_url)
            } else {
                vec![0x00]
            };
            output.push(Bytes::from(block));
            *bytes_since_meta = 0;
        }
    }
    output
}

/// Inline replacement for `tokio_stream::wrappers::UnboundedReceiverStream`.
struct UnboundedReceiverStream<T> {
    rx: UnboundedReceiver<T>,
}

impl<T> UnboundedReceiverStream<T> {
    fn new(rx: UnboundedReceiver<T>) -> Self {
        Self { rx }
    }
}

impl<T> Stream for UnboundedReceiverStream<T> {
    type Item = T;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        self.get_mut().rx.poll_recv(cx)
    }
}

/// Render the info page HTML from a minijinja-style template using simple string replacement.
fn render_info_page(ctx: &InfoPageContext, template: &str) -> String {
    let mut html = template.to_owned();

    // Process {% for stream in streams %}...{% endfor %} blocks
    while let Some(start) = html.find("{% for stream in streams %}") {
        let body_start = start + "{% for stream in streams %}".len();
        let end = html[body_start..]
            .find("{% endfor %}")
            .map(|p| body_start + p)
            .expect("missing {% endfor %} in template");
        let inner = html[body_start..end].to_owned();

        let mut replacement = String::new();
        for stream in &ctx.streams {
            replacement.push_str(
                &inner
                    .replace("{{ stream.name }}", &stream.name)
                    .replace("{{ stream.bitrate }}", &stream.bitrate.to_string())
                    .replace("{{ stream.url }}", &stream.url),
            );
        }

        let endfor_end = end + "{% endfor %}".len();
        html.replace_range(start..endfor_end, &replacement);
    }

    html.replace("{{ station_name }}", &ctx.station_name)
        .replace("{{ first_stream }}", &ctx.first_stream)
        .replace("{{ station_genre }}", &ctx.station_genre)
        .replace("{{ station_description }}", &ctx.station_description)
        .replace("{{ current_track }}", &ctx.current_track)
        .replace("{{ album }}", &ctx.album)
        .replace("{{ bitrate }}", &ctx.bitrate.to_string())
        .replace("{{ public_url }}", &ctx.public_url)
        .replace("{{ cover_url }}", &ctx.cover_url)
        .replace("{{ version }}", &ctx.version)
        .replace("{{ listeners }}", &ctx.listeners.to_string())
        .replace("{{ uptime }}", &ctx.uptime)
}

impl IcecastServer {
    pub fn new(
        stream_buffers: Vec<StreamBufferEntry>,
        station_name: String,
        station_description: String,
        station_genre: String,
        timeline_rx: watch::Receiver<TimelineSnapshot>,
        public_url: Option<String>,
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
            public_url,
            port: 0,
            start_time: Instant::now(),
        }
    }

    pub async fn start_server(
        &self,
        public_url: Option<String>,
        port: u16,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if public_url.is_none() {
            log::warn!(
                "No public_url configured - album art and stream URLs will not be accessible from external clients"
            );
        }

        let server = Arc::new(self.clone());

        // Build cover URL from public_url or set to None if not configured
        let cover_url: Option<String> = public_url.as_ref().map(|url| format!("{}/cover.jpg", url));

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
                let cover_url = cover_url.clone();

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
                                cover_url: cover_url.clone(),
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

        let favicon_bytes: &'static [u8] = include_bytes!("../favicon.ico");
        let favicon_route = warp::path("favicon.ico")
            .and(warp::get())
            .map(move || warp::reply::with_header(favicon_bytes, "Content-Type", "image/x-icon"));

        let current_route = warp::path("current").and(warp::get()).and_then({
            let server = Arc::clone(&server);
            move || {
                let server = Arc::clone(&server);
                async move { server.handle_current_request().await }
            }
        });

        let cover_route = warp::path("cover.jpg").and(warp::get()).and_then({
            let server = Arc::clone(&server);
            move || {
                let server = Arc::clone(&server);
                async move { server.handle_cover_request().await }
            }
        });

        // Swagger API documentation routes
        let swagger_ui_route = server_swagger::swagger_ui();
        let openapi_spec_route = server_swagger::openapi_spec();

        let routes = stream_route
            .or(status_route)
            .or(current_route)
            .or(cover_route)
            .or(swagger_ui_route)
            .or(openapi_spec_route)
            .or(info_route)
            .or(favicon_route);
        let bind_addr = "0.0.0.0";
        if let Some(ref url) = public_url {
            log::info!("Starting Funkstrom server on {}", url);
            log::info!("API Docs: {}/api-docs", url);
        } else {
            log::info!("Starting Funkstrom server on {}:{}", bind_addr, port);
        }

        let addr: std::net::SocketAddr = format!("{}:{}", bind_addr, port).parse()?;
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
        let cover_url = context.cover_url.clone();
        let do_icy_metadata_clone = do_icy_metadata;

        const METADATA_INTERVAL: usize = 16000;
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
                            cover_url.as_deref(),
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
            response_builder =
                response_builder.header("icy-metaint", format!("{}", METADATA_INTERVAL));
        }

        let response = response_builder
            .body(warp::hyper::Body::wrap_stream(stream))
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
            version: VERSION.to_string(),
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

    async fn handle_cover_request(&self) -> Result<impl Reply, warp::Rejection> {
        let snapshot = self.timeline_rx.borrow().clone();
        let path = &snapshot.track_path;

        // Guard against empty path (no track playing)
        if path.as_os_str().is_empty() {
            return warp::http::Response::builder()
                .status(warp::http::StatusCode::NO_CONTENT)
                .body(vec![])
                .map_err(|_| warp::reject::reject());
        }

        match extract_cover_from_file(path) {
            Some((data, mime)) => {
                let response = warp::http::Response::builder()
                    .header("Content-Type", mime)
                    .header("Cache-Control", "public, max-age=60")
                    .body(data)
                    .map_err(|_| warp::reject::reject())?;
                Ok(response)
            }
            None => Ok(warp::http::Response::builder()
                .status(warp::http::StatusCode::NO_CONTENT)
                .body(vec![])
                .map_err(|_| warp::reject::reject())?),
        }
    }

    async fn handle_info_request(&self) -> Result<impl Reply, warp::Rejection> {
        let snapshot = self.timeline_rx.borrow().clone();
        let current_track = snapshot.current_metadata.to_icy_metadata();
        let album = &snapshot.current_metadata.album;

        let base_url = self
            .public_url
            .clone()
            .unwrap_or_else(|| format!("http://0.0.0.0:{}", self.port));

        // Build streams list for template context
        let streams: Vec<StreamLink> = self
            .streams
            .iter()
            .map(|stream| StreamLink {
                name: stream.name.clone(),
                bitrate: stream.bitrate,
                url: format!("{}/{}", base_url, stream.name),
            })
            .collect();

        // Use the first stream for the audio player
        let first_stream = self
            .streams
            .first()
            .map(|s| s.name.clone())
            .unwrap_or_else(|| "stream".to_string());
        let first_bitrate = self.streams.first().map(|s| s.bitrate).unwrap_or(128);
        let total_listeners: usize = self
            .streams
            .iter()
            .map(|s| s.listeners.load(Ordering::SeqCst))
            .sum();

        let uptime = {
            let elapsed = self.start_time.elapsed().as_secs();
            format!("{}h {}m", elapsed / 3600, (elapsed % 3600) / 60)
        };

        let context = InfoPageContext {
            station_name: self.station_name.clone(),
            current_track,
            album: album.clone(),
            station_description: self.station_description.clone(),
            station_genre: self.station_genre.clone(),
            bitrate: first_bitrate,
            public_url: base_url.clone(),
            streams,
            first_stream,
            cover_url: format!("{}/cover.jpg", base_url),
            version: VERSION.to_string(),
            listeners: total_listeners,
            uptime,
        };

        const TEMPLATE_STR: &str = include_str!("../templates/info.html");
        let rendered = render_info_page(&context, TEMPLATE_STR);

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
        let block = build_icy_metadata_block("Artist - Title", None);
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
        let block = build_icy_metadata_block("AB", None);
        assert_eq!(block[0], 2);
        assert_eq!(block.len(), 1 + 2 * 16);
    }

    #[test]
    fn given_empty_title_when_building_metadata_block_then_still_valid() {
        let block = build_icy_metadata_block("", None);
        assert_eq!(block[0], 2); // "StreamTitle='';StreamUrl='';" = 30 chars → ceil(30/16)=2
        assert!(block[1..].iter().any(|&b| b != 0)); // has content
    }

    #[test]
    fn given_very_long_title_when_building_metadata_block_then_clamped_to_255_blocks() {
        let long = "X".repeat(5000);
        let block = build_icy_metadata_block(&long, None);
        assert_eq!(block[0], 255); // clamped
        assert_eq!(block.len(), 1 + 255 * 16); // 4081 bytes
    }

    #[test]
    fn given_special_characters_when_building_metadata_block_then_utf8_preserved() {
        let block = build_icy_metadata_block("Motörhead - Åce of Spädes", None);
        let content = String::from_utf8_lossy(&block[1..]);
        assert!(content.contains("Motörhead"));
        assert!(content.contains("Åce of Spädes"));
    }

    #[test]
    fn given_stream_url_when_building_metadata_block_then_url_included() {
        let block =
            build_icy_metadata_block("Artist - Title", Some("http://example.com/cover.jpg"));
        let content = String::from_utf8_lossy(&block[1..]);
        assert!(content.contains("StreamTitle='Artist - Title';"));
        assert!(content.contains("StreamUrl='http://example.com/cover.jpg';"));
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
    );

    assert!(blocks.is_empty());
    assert_eq!(bytes_since_meta, 0);
    assert_eq!(last_meta_str, "");
}

// --- extract_cover_from_file tests ---

#[test]
fn given_file_without_cover_when_extracting_cover_then_returns_none() {
    use std::io::Write;
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    // Write minimal valid MP3 without ID3 tags (or with empty tags)
    // audiotags will fail to find tags, so album() returns None
    tmp.write_all(b"\xFF\xFB\x90\x00").unwrap();
    let result = extract_cover_from_file(tmp.path());
    assert!(result.is_none());
}

#[test]
fn given_file_with_cover_when_extracting_cover_then_returns_image() {
    // Minimal 1x1 white JPEG
    let jpeg_data: &[u8] = &[
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00, 0x00,
        0x01, 0x00, 0x01, 0x00, 0x00, 0xFF, 0xDB, 0x00, 0x43, 0x00, 0x08, 0x06, 0x06, 0x07, 0x06,
        0x05, 0x08, 0x07, 0x07, 0x07, 0x09, 0x09, 0x08, 0x0A, 0x0C, 0x14, 0x0D, 0x0C, 0x0B, 0x0B,
        0x0C, 0x19, 0x12, 0x13, 0x0F, 0x14, 0x1D, 0x1A, 0x1F, 0x1E, 0x1D, 0x1A, 0x1C, 0x1C, 0x20,
        0x24, 0x2E, 0x27, 0x20, 0x22, 0x2C, 0x23, 0x1C, 0x1C, 0x28, 0x37, 0x29, 0x2C, 0x30, 0x31,
        0x34, 0x34, 0x34, 0x1F, 0x27, 0x39, 0x3D, 0x38, 0x32, 0x3C, 0x2E, 0x33, 0x34, 0x32, 0xFF,
        0xC0, 0x00, 0x0B, 0x08, 0x00, 0x01, 0x00, 0x01, 0x01, 0x01, 0x11, 0x00, 0xFF, 0xC4, 0x00,
        0x1F, 0x00, 0x00, 0x01, 0x05, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B,
        0xFF, 0xC4, 0x00, 0xB5, 0x10, 0x00, 0x02, 0x01, 0x03, 0x03, 0x02, 0x04, 0x03, 0x05, 0x05,
        0x04, 0x04, 0x00, 0x00, 0x01, 0x7D, 0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21,
        0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1, 0x08,
        0x23, 0x42, 0xB1, 0xC1, 0x15, 0x52, 0xD1, 0xF0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0A,
        0x16, 0x17, 0x18, 0x19, 0x1A, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x34, 0x35, 0x36, 0x37,
        0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55, 0x56,
        0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x73, 0x74, 0x75,
        0x76, 0x77, 0x78, 0x79, 0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8A, 0x92, 0x93,
        0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9,
        0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6,
        0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2,
        0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7,
        0xF8, 0xF9, 0xFA, 0xFF, 0xDA, 0x00, 0x0C, 0x03, 0x01, 0x00, 0x02, 0x11, 0x03, 0x11, 0x00,
        0x3F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF,
        0xD9,
    ];

    // Build a valid MP3 file with an ID3v2 tag containing cover art,
    // using the `id3` crate (already a transitive dependency via audiotags).
    use id3::frame::{Content, Picture, PictureType};
    use id3::{Frame, TagLike};

    let mut tag = id3::Tag::new();
    let picture = Picture {
        mime_type: "image/jpeg".to_string(),
        picture_type: PictureType::CoverFront,
        description: "cover".to_string(),
        data: jpeg_data.to_vec(),
    };
    tag.add_frame(Frame::with_content("APIC", Content::Picture(picture)));
    // audiotags requires a TALB frame for album() to return Some
    tag.set_album("Test Album");

    let mut mp3_bytes = Vec::new();
    tag.write_to(&mut mp3_bytes, id3::Version::Id3v23)
        .expect("failed to write ID3 tag");
    mp3_bytes.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
    mp3_bytes.resize(mp3_bytes.len() + 100, 0x00);
    // audiotags detects format by file extension, so use .mp3
    let tmpdir = tempfile::tempdir().unwrap();
    let audio_path = tmpdir.path().join("test.mp3");
    std::fs::write(&audio_path, &mp3_bytes).unwrap();

    let result = extract_cover_from_file(&audio_path);
    assert!(result.is_some(), "expected cover art to be extracted");
    let (_data, mime_type) = result.unwrap();
    assert_eq!(mime_type, "image/jpeg");
}

// --- process_audio_with_icy StreamUrl test ---

#[test]
fn given_stream_url_when_processing_audio_then_metadata_block_contains_stream_url() {
    let chunk = Bytes::from(vec![0u8; 32000]); // spans two 16000 boundaries
    let mut bytes_since_meta: usize = 0;
    let mut last_meta_str = String::new();
    let stream_url = Some("http://example.com/cover.jpg");

    let blocks = process_audio_with_icy(
        chunk,
        16000,
        &mut bytes_since_meta,
        &mut last_meta_str,
        "Artist - Title",
        stream_url,
    );

    // We should have at least one metadata block
    assert!(!blocks.is_empty());
    // Find the metadata block (the one starting with a non-zero count byte)
    let meta_block = blocks
        .iter()
        .find(|b| b.len() > 1 && b[0] > 0)
        .expect("expected a metadata block");
    let content = String::from_utf8_lossy(&meta_block[1..]);
    assert!(
        content.contains("StreamUrl='http://example.com/cover.jpg'"),
        "expected StreamUrl in metadata, got: {}",
        content
    );
}

// --- render_info_page tests ---

fn info_page_context_with_streams(streams: Vec<StreamLink>) -> InfoPageContext {
    InfoPageContext {
        station_name: "Funkstrom FM".to_string(),
        current_track: "Artist - Track".to_string(),
        album: "The Album".to_string(),
        station_description: "A radio station".to_string(),
        station_genre: "Electronic".to_string(),
        bitrate: 320,
        public_url: "https://radio.example.com".to_string(),
        streams,
        first_stream: "/stream".to_string(),
        cover_url: "https://radio.example.com/cover.jpg".to_string(),
        version: "1.2.3".to_string(),
        listeners: 42,
        uptime: "1h 2m".to_string(),
    }
}

#[test]
fn given_template_with_loop_when_two_streams_then_body_repeated_per_stream() {
    let ctx = info_page_context_with_streams(vec![
        StreamLink {
            name: "lo".to_string(),
            bitrate: 128,
            url: "/low".to_string(),
        },
        StreamLink {
            name: "hi".to_string(),
            bitrate: 320,
            url: "/high".to_string(),
        },
    ]);
    let template = "A{% for stream in streams %}({{ stream.name }}|{{ stream.bitrate }}|{{ stream.url }}){% endfor %}B";

    let rendered = render_info_page(&ctx, template);

    assert_eq!(rendered, "A(lo|128|/low)(hi|320|/high)B");
}

#[test]
fn given_template_with_loop_when_no_streams_then_loop_block_removed_entirely() {
    let ctx = info_page_context_with_streams(Vec::new());
    let template = "A{% for stream in streams %}({{ stream.name }}){% endfor %}B";

    let rendered = render_info_page(&ctx, template);

    assert_eq!(rendered, "AB");
}

#[test]
fn given_stream_placeholders_in_loop_when_single_stream_then_all_substituted() {
    let ctx = info_page_context_with_streams(vec![StreamLink {
        name: "main".to_string(),
        bitrate: 256,
        url: "https://radio.example.com/main".to_string(),
    }]);
    let template =
        "{% for stream in streams %}<a href=\"{{ stream.url }}\">{{ stream.name }} {{ stream.bitrate }}</a>{% endfor %}";

    let rendered = render_info_page(&ctx, template);

    assert_eq!(
        rendered,
        "<a href=\"https://radio.example.com/main\">main 256</a>"
    );
}

#[test]
fn given_scalar_placeholders_when_rendering_then_each_substituted_with_context_value() {
    let ctx = info_page_context_with_streams(Vec::new());
    let template =
        "{{ station_name }}|{{ first_stream }}|{{ station_genre }}|{{ station_description }}\
                    |{{ current_track }}|{{ album }}|{{ bitrate }}|{{ public_url }}|{{ cover_url }}\
                    |{{ version }}|{{ listeners }}|{{ uptime }}";

    let rendered = render_info_page(&ctx, template);

    assert_eq!(
        rendered,
        "Funkstrom FM|/stream|Electronic|A radio station|Artist - Track|The Album|320|https://radio.example.com\
         |https://radio.example.com/cover.jpg|1.2.3|42|1h 2m"
    );
}
