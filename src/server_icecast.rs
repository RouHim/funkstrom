use crate::fanout_buffer::SharedFanoutBuffer;
use crate::playback_director::TimelineSnapshot;
use crate::server_swagger;

use futures_core::Stream;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::{mpsc, watch};
use warp::{http::HeaderMap, Filter, Reply};

pub(crate) use crate::icy::{
    extract_cover_from_file, process_audio_with_icy, render_info_page, InfoPageContext,
    StatusResponse, StreamLink, StreamStatus,
};

// JSON response structures for serialization
const VERSION: &str = include_str!(concat!(env!("OUT_DIR"), "/version.txt"));

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

fn split_hms(total_secs: u64) -> (u64, u64, u64) {
    (
        (total_secs / 3600),
        (total_secs % 3600) / 60,
        total_secs % 60,
    )
}

/// Inline replacement for `tokio_stream::wrappers::UnboundedReceiverStream`.
struct UnboundedReceiverStream<T> {
    rx: Mutex<UnboundedReceiver<T>>,
}

impl<T> UnboundedReceiverStream<T> {
    fn new(rx: UnboundedReceiver<T>) -> Self {
        Self { rx: Mutex::new(rx) }
    }
}

impl<T> Stream for UnboundedReceiverStream<T> {
    type Item = T;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let mut rx = self.get_mut().rx.lock().expect("receiver mutex poisoned");
        rx.poll_recv(cx)
    }
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

        let status_route = warp::path("status".to_owned()).and(warp::get()).and_then({
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

        let favicon_bytes: Vec<u8> = include_bytes!("../favicon.ico").to_vec();
        let favicon_route = warp::path("favicon.ico".to_owned())
            .and(warp::get())
            .map(move || {
                warp::reply::with_header(favicon_bytes.clone(), "Content-Type", "image/x-icon")
            });

        let current_route = warp::path("current".to_owned()).and(warp::get()).and_then({
            let server = Arc::clone(&server);
            move || {
                let server = Arc::clone(&server);
                async move { server.handle_current_request().await }
            }
        });

        let cover_route = warp::path("cover.jpg".to_owned())
            .and(warp::get())
            .and_then({
                let server = Arc::clone(&server);
                move || {
                    let server = Arc::clone(&server);
                    async move { server.handle_cover_request().await }
                }
            });

        // Swagger API documentation routes
        let swagger_ui_route = server_swagger::swagger_ui();
        let swagger_css_route = server_swagger::swagger_css();
        let swagger_js_route = server_swagger::swagger_bundle_js();
        let openapi_spec_route = server_swagger::openapi_spec();

        let routes = stream_route
            .or(status_route)
            .or(current_route)
            .or(cover_route)
            .or(swagger_ui_route)
            .or(swagger_css_route)
            .or(swagger_js_route)
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
            .body(warp::reply::stream(stream).into_response().into_body())
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
                let (h, m, s) = split_hms(self.start_time.elapsed().as_secs());
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
            let (h, m, _) = split_hms(self.start_time.elapsed().as_secs());
            format!("{}h {}m", h, m)
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
}
