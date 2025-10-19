use crate::audio_buffer::StreamBuffer;
use crate::audio_metadata::TrackMetadata;
use crate::server_swagger;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use warp::{http::HeaderMap, Filter, Reply};

#[derive(Clone)]
pub struct IcecastServer {
    buffer: StreamBuffer,
    station_name: String,
    station_description: String,
    station_genre: String,
    bitrate: u32,
    current_metadata: Arc<Mutex<TrackMetadata>>,
    bind_address: Arc<Mutex<String>>,
    port: Arc<Mutex<u16>>,
}

impl IcecastServer {
    pub fn new(
        buffer: StreamBuffer,
        station_name: String,
        station_description: String,
        station_genre: String,
        bitrate: u32,
        current_metadata: Arc<Mutex<TrackMetadata>>,
    ) -> Self {
        Self {
            buffer,
            station_name,
            station_description,
            station_genre,
            bitrate,
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

        let stream_route = warp::path("stream")
            .and(warp::get())
            .and(warp::header::headers_cloned())
            .and_then({
                let server = Arc::clone(&server);
                move |headers: HeaderMap| {
                    let server = Arc::clone(&server);
                    async move { server.handle_stream_request(headers).await }
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

        log::info!("Starting Icecast server on {}:{}", bind_address, port);
        log::info!("Stream URL: http://{}:{}/stream", bind_address, port);
        log::info!("API Docs: http://{}:{}/api-docs", bind_address, port);

        let addr: std::net::SocketAddr = format!("{}:{}", bind_address, port)
            .parse()
            .expect("Invalid bind address");
        warp::serve(routes).run(addr).await;
    }

    async fn handle_stream_request(
        &self,
        headers: HeaderMap,
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
        let buffer = self.buffer.clone();

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
            .header("icy-name", &self.station_name)
            .header("icy-description", &self.station_description)
            .header("icy-genre", &self.station_genre)
            .header("icy-br", self.bitrate.to_string())
            .header("icy-metaint", "16000")
            .header("Server", &server_version)
            .body(hyper::Body::wrap_stream(stream))
            .unwrap();

        Ok(response)
    }

    async fn handle_status_request(&self) -> Result<impl Reply, warp::Rejection> {
        let (chunks, bytes) = self.buffer.buffer_info();
        let is_running = self.buffer.is_running();

        let status = format!(
            r#"{{
    "status": "{}",
    "station_name": "{}",
    "station_description": "{}",
    "station_genre": "{}",
    "bitrate": {},
    "buffer_chunks": {},
    "buffer_bytes": {},
    "uptime": "unknown"
}}"#,
            if is_running { "online" } else { "offline" },
            self.station_name,
            self.station_description,
            self.station_genre,
            self.bitrate,
            chunks,
            bytes
        );

        Ok(warp::reply::with_header(
            status,
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

        const TEMPLATE: &str = include_str!("../templates/info.html");

        let info = TEMPLATE
            .replace("{station_name}", &self.station_name)
            .replace("{current_track}", &current_track)
            .replace("{album}", album)
            .replace("{station_description}", &self.station_description)
            .replace("{station_genre}", &self.station_genre)
            .replace("{bitrate}", &self.bitrate.to_string())
            .replace("{bind_address}", &bind_address)
            .replace("{port}", &port.to_string());

        Ok(warp::reply::with_header(
            info,
            "Content-Type",
            "text/html; charset=utf-8",
        ))
    }
}
