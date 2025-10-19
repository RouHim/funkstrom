use crate::audio_buffer::StreamBuffer;
use crate::audio_metadata::TrackMetadata;
use crate::server_swagger;
use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tinytemplate::TinyTemplate;
use tokio::sync::mpsc;
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

// Context for handling stream requests
#[derive(Clone)]
struct StreamContext {
    buffer: StreamBuffer,
    bitrate: u32,
    station_name: String,
    station_description: String,
    station_genre: String,
}

#[derive(Clone)]
pub struct IcecastServer {
    streams: Arc<Vec<StreamEndpoint>>,
    station_name: String,
    station_description: String,
    station_genre: String,
    current_metadata: Arc<Mutex<TrackMetadata>>,
    bind_address: Arc<Mutex<String>>,
    port: Arc<Mutex<u16>>,
}

#[derive(Clone)]
struct StreamEndpoint {
    name: String,
    buffer: StreamBuffer,
    bitrate: u32,
}

impl IcecastServer {
    pub fn new(
        stream_buffers: Vec<(String, StreamBuffer, u32)>,
        station_name: String,
        station_description: String,
        station_genre: String,
        current_metadata: Arc<Mutex<TrackMetadata>>,
    ) -> Self {
        let streams = stream_buffers
            .into_iter()
            .map(|(name, buffer, bitrate)| StreamEndpoint {
                name,
                buffer,
                bitrate,
            })
            .collect();

        Self {
            streams: Arc::new(streams),
            station_name,
            station_description,
            station_genre,
            current_metadata,
            bind_address: Arc::new(Mutex::new(String::new())),
            port: Arc::new(Mutex::new(0)),
        }
    }

    pub async fn start_server(&self, bind_address: &str, port: u16) {
        // Store bind_address and port for use in info page
        *self.bind_address.lock().unwrap() = bind_address.to_string();
        *self.port.lock().unwrap() = port;

        let server = Arc::new(self.clone());

        // Dynamic stream route handler
        let streams_map = self.streams.clone();
        let station_name = self.station_name.clone();
        let station_description = self.station_description.clone();
        let station_genre = self.station_genre.clone();

        let stream_route = warp::path::param::<String>()
            .and(warp::get())
            .and(warp::header::headers_cloned())
            .and_then(move |stream_name: String, headers: HeaderMap| {
                let streams = streams_map.clone();
                let station_name = station_name.clone();
                let station_description = station_description.clone();
                let station_genre = station_genre.clone();

                async move {
                    // Find the stream by name and create context
                    for stream in streams.iter() {
                        if stream.name == stream_name {
                            let context = StreamContext {
                                buffer: stream.buffer.clone(),
                                bitrate: stream.bitrate,
                                station_name: station_name.clone(),
                                station_description: station_description.clone(),
                                station_genre: station_genre.clone(),
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

        let addr: std::net::SocketAddr = format!("{}:{}", bind_address, port)
            .parse()
            .expect("Invalid bind address");
        warp::serve(routes).run(addr).await;
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

        let (tx, rx) = mpsc::unbounded_channel();
        let buffer = context.buffer.clone();

        tokio::spawn(async move {
            let mut last_data_time = Instant::now();
            let timeout_duration = Duration::from_secs(30);

            loop {
                if let Some(chunk) = buffer.read_chunk(8192) {
                    if tx.send(Ok::<_, warp::Error>(chunk)).is_err() {
                        log::info!("Client disconnected");
                        break;
                    }
                    last_data_time = Instant::now();
                } else {
                    if last_data_time.elapsed() > timeout_duration {
                        log::warn!("No data available for too long, disconnecting client");
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        });

        let stream = UnboundedReceiverStream::new(rx);

        let server_version = format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));

        let response = warp::http::Response::builder()
            .header("Content-Type", "audio/mpeg")
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
            .header("icy-metaint", "16000")
            .header("Server", &server_version)
            .body(hyper::Body::wrap_stream(stream))
            .unwrap();

        Ok(response)
    }

    async fn handle_status_request(&self) -> Result<impl Reply, warp::Rejection> {
        let streams = self
            .streams
            .iter()
            .map(|stream| {
                let (chunks, bytes) = stream.buffer.buffer_info();
                let is_running = stream.buffer.is_running();
                StreamStatus {
                    name: stream.name.clone(),
                    bitrate: stream.bitrate,
                    status: if is_running {
                        "online".to_string()
                    } else {
                        "offline".to_string()
                    },
                    buffer_chunks: chunks,
                    buffer_bytes: bytes,
                }
            })
            .collect();

        let response = StatusResponse {
            station_name: self.station_name.clone(),
            station_description: self.station_description.clone(),
            station_genre: self.station_genre.clone(),
            streams,
            uptime: "unknown".to_string(),
        };

        let json = serde_json::to_string(&response).unwrap();

        Ok(warp::reply::with_header(
            json,
            "Content-Type",
            "application/json",
        ))
    }

    async fn handle_current_request(&self) -> Result<impl Reply, warp::Rejection> {
        let metadata = self.current_metadata.lock().unwrap();
        let json = metadata.to_json();

        Ok(warp::reply::with_header(
            json,
            "Content-Type",
            "application/json",
        ))
    }

    async fn handle_info_request(&self) -> Result<impl Reply, warp::Rejection> {
        let metadata = self.current_metadata.lock().unwrap();
        let current_track = metadata.to_icy_metadata();
        let album = &metadata.album;

        let bind_address = self.bind_address.lock().unwrap().clone();
        let port = *self.port.lock().unwrap();

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

        let mut tt = TinyTemplate::new();
        tt.add_template("info", TEMPLATE_STR).map_err(|e| {
            log::error!("Template error: {}", e);
            warp::reject::reject()
        })?;

        let rendered = tt.render("info", &context).map_err(|e| {
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
