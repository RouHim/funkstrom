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
        }
    }

    pub async fn start_server(&self, port: u16) {
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

        println!("Starting Icecast server on port {}", port);
        println!("Stream URL: http://127.0.0.1:{}/stream", port);
        println!("API Docs: http://127.0.0.1:{}/api-docs", port);

        warp::serve(routes).run(([127, 0, 0, 1], port)).await;
    }

    async fn handle_stream_request(
        &self,
        headers: HeaderMap,
    ) -> Result<impl Reply, warp::Rejection> {
        println!("New client connected for streaming");

        let user_agent = headers
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("Unknown");

        println!("Client User-Agent: {}", user_agent);

        let (tx, rx) = mpsc::unbounded_channel();
        let buffer = self.buffer.clone();

        tokio::spawn(async move {
            let mut last_data_time = Instant::now();
            let timeout_duration = Duration::from_secs(30);

            loop {
                if let Some(chunk) = buffer.read_chunk(8192) {
                    if tx.send(Ok::<_, warp::Error>(chunk)).is_err() {
                        println!("Client disconnected");
                        break;
                    }
                    last_data_time = Instant::now();
                } else {
                    if last_data_time.elapsed() > timeout_duration {
                        println!("No data available for too long, disconnecting client");
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        });

        let stream = UnboundedReceiverStream::new(rx);

        let response = warp::http::Response::builder()
            .header("Content-Type", "audio/mpeg")
            .header("Cache-Control", "no-cache, no-store")
            .header("Connection", "close")
            .header("Pragma", "no-cache")
            .header("Access-Control-Allow-Origin", "*")
            .header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
            .header("Access-Control-Allow-Headers", "Content-Type")
            .header("icy-name", &self.station_name)
            .header("icy-description", &self.station_description)
            .header("icy-genre", &self.station_genre)
            .header("icy-br", self.bitrate.to_string())
            .header("icy-metaint", "16000")
            .header("Server", "Funkstrom/1.0")
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

        let info = format!(
            r#"<!DOCTYPE html>
<html>
<head>
    <title>{} - Funkstrom</title>
    <style>
        body {{ font-family: Arial, sans-serif; margin: 40px; background: #f5f5f5; }}
        .container {{ max-width: 600px; margin: 0 auto; background: white; padding: 30px; border-radius: 8px; box-shadow: 0 2px 10px rgba(0,0,0,0.1); }}
        h1 {{ color: #333; border-bottom: 2px solid #4CAF50; padding-bottom: 10px; }}
        .info {{ background: #f9f9f9; padding: 15px; border-radius: 5px; margin: 20px 0; }}
        .stream-link {{ background: #4CAF50; color: white; padding: 10px 20px; text-decoration: none; border-radius: 5px; display: inline-block; margin: 10px 0; }}
        .stream-link:hover {{ background: #45a049; }}
        .status {{ font-weight: bold; color: #4CAF50; }}
        .now-playing {{ background: #e8f5e9; padding: 20px; border-radius: 5px; margin: 20px 0; border-left: 4px solid #4CAF50; }}
        .now-playing h2 {{ margin-top: 0; color: #2e7d32; }}
        .track-info {{ font-size: 1.1em; margin: 10px 0; }}
    </style>
</head>
<body>
    <div class="container">
        <h1>{}</h1>
        <div class="now-playing">
            <h2>Now Playing</h2>
            <div class="track-info">{}</div>
            <div style="color: #666; margin-top: 5px;">Album: {}</div>
        </div>
        <div class="info">
            <p><strong>Description:</strong> {}</p>
            <p><strong>Genre:</strong> {}</p>
            <p><strong>Bitrate:</strong> {} kbps</p>
            <p><strong>Status:</strong> <span class="status">Online</span></p>
        </div>
        <a href="/stream" class="stream-link">🎵 Listen Now</a>
        <a href="/status" class="stream-link">📊 Status (JSON)</a>
        <a href="/current" class="stream-link">🎵 Current Track (JSON)</a>
        <a href="/api-docs" class="stream-link">📖 API Documentation</a>
        <div class="info">
            <h3>How to listen:</h3>
            <p>Copy this URL into your favorite media player:</p>
            <code>http://127.0.0.1:8000/stream</code>
            <p><small>Compatible with VLC, Winamp, iTunes, and most other media players.</small></p>
        </div>
    </div>
</body>
</html>"#,
            self.station_name,
            self.station_name,
            current_track,
            album,
            self.station_description,
            self.station_genre,
            self.bitrate
        );

        Ok(warp::reply::with_header(
            info,
            "Content-Type",
            "text/html; charset=utf-8",
        ))
    }
}
