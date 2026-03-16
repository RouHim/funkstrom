use bytes::Bytes;
use crossbeam_channel::{unbounded, Receiver, Sender};
use log::{debug, error, info, warn};
use std::io::ErrorKind;
use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{watch, Notify};

use crate::playback_director::TimelineSnapshot;

// Constants for audio processing configuration
const AUDIO_CHUNK_SIZE: usize = 8192; // 8KB chunks for reading audio data
const PROCESS_POLL_INTERVAL_MS: u64 = 10; // How often to poll FFmpeg process
const INITIAL_BACKOFF_MS: u64 = 1000;
const MAX_BACKOFF_MS: u64 = 5_000;
#[allow(dead_code)]
const IDLE_GRACE_PERIOD_SECS: u64 = 60;

fn looks_like_filesystem_path(value: &str) -> bool {
    value.starts_with('/') || value.contains('/')
}

fn resolve_ffmpeg_path(configured_path: Option<String>, env_ffmpeg_path: Option<String>) -> String {
    let configured_path = configured_path
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty());

    if let Some(path) = configured_path.as_ref() {
        if !looks_like_filesystem_path(path) || Path::new(path).exists() {
            return path.clone();
        }
        warn!(
            "Configured FFmpeg path '{}' does not exist, trying fallbacks",
            path
        );
    }

    let env_ffmpeg_path = env_ffmpeg_path
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty());

    if let Some(path) = env_ffmpeg_path {
        if !looks_like_filesystem_path(&path) || Path::new(&path).exists() {
            return path;
        }
        warn!(
            "FFMPEG_PATH '{}' does not exist, trying built-in fallbacks",
            path
        );
    }

    let container_ffmpeg = "/ffmpeg";
    if Path::new(container_ffmpeg).exists() {
        return container_ffmpeg.to_string();
    }

    "ffmpeg".to_string()
}

fn calculate_backoff_ms(consecutive_failures: u32) -> u64 {
    let backoff =
        INITIAL_BACKOFF_MS.saturating_mul(1u64.wrapping_shl(consecutive_failures.min(15)));
    backoff.min(MAX_BACKOFF_MS)
}

fn is_url_input(input: &str) -> bool {
    input.starts_with("http://") || input.starts_with("https://")
}

pub struct FFmpegProcessor {
    ffmpeg_path: String,
    sample_rate: u32,
    bitrate: u32,
    channels: u8,
    format: String,
}

impl FFmpegProcessor {
    pub fn new(
        ffmpeg_path: Option<String>,
        sample_rate: u32,
        bitrate: u32,
        channels: u8,
        format: String,
    ) -> Self {
        let ffmpeg_path = resolve_ffmpeg_path(ffmpeg_path, std::env::var("FFMPEG_PATH").ok());
        debug!("Using FFmpeg executable: {}", ffmpeg_path);

        Self {
            ffmpeg_path,
            sample_rate,
            bitrate,
            channels,
            format,
        }
    }

    fn get_codec_for_format(&self, format: &str) -> &str {
        match format {
            "mp3" => "libmp3lame",
            "opus" => "libopus",
            "aac" => "aac",
            "vorbis" | "ogg" => "libvorbis",
            "flac" => "flac",
            _ => {
                warn!("Unknown format '{}', defaulting to libmp3lame", format);
                "libmp3lame"
            }
        }
    }

    fn get_muxer_for_format(&self, format: &str) -> &str {
        match format {
            "aac" => "adts",
            "mp3" => "mp3",
            "opus" => "opus",
            "vorbis" | "ogg" => "ogg",
            "flac" => "flac",
            _ => {
                warn!("Unknown format '{}', defaulting to mp3 muxer", format);
                "mp3"
            }
        }
    }

    pub fn check_ffmpeg_available(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        debug!("Checking FFmpeg availability at: {}", self.ffmpeg_path);

        let output = match Command::new(&self.ffmpeg_path).args(["-version"]).output() {
            Ok(output) => output,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                return Err(format!(
                    "FFmpeg executable '{}' was not found. Configure [server].ffmpeg_path or set FFMPEG_PATH.",
                    self.ffmpeg_path
                )
                .into())
            }
            Err(err) => {
                return Err(format!(
                    "Failed to execute FFmpeg '{}' for startup validation: {}",
                    self.ffmpeg_path, err
                )
                .into())
            }
        };

        if !output.status.success() {
            return Err(format!("FFmpeg not found at path: {}", self.ffmpeg_path).into());
        }

        let version_info = String::from_utf8_lossy(&output.stdout);
        info!(
            "FFmpeg available: {}",
            version_info.lines().next().unwrap_or("Unknown version")
        );

        Ok(())
    }

    #[allow(dead_code)]
    pub fn start_conversion_process(
        &self,
        input_path: &Path,
    ) -> Result<AudioProcess, Box<dyn std::error::Error + Send + Sync>> {
        let input_str = input_path.to_str().ok_or_else(|| {
            format!(
                "Path contains invalid UTF-8 characters: {}",
                input_path.display()
            )
        })?;
        self.start_conversion(input_str)
    }

    #[allow(dead_code)]
    pub fn start_conversion_from_url(
        &self,
        url: &str,
    ) -> Result<AudioProcess, Box<dyn std::error::Error + Send + Sync>> {
        self.start_conversion(url)
    }

    #[allow(dead_code)]
    pub fn start_conversion_with_seek(
        &self,
        input: &str,
        seek_offset_secs: Option<f64>,
    ) -> Result<AudioProcess, Box<dyn std::error::Error + Send + Sync>> {
        self.start_conversion_internal(input, seek_offset_secs)
    }

    #[allow(dead_code)]
    fn start_conversion(
        &self,
        input: &str,
    ) -> Result<AudioProcess, Box<dyn std::error::Error + Send + Sync>> {
        self.start_conversion_internal(input, None)
    }

    fn build_ffmpeg_args(&self, input: &str, seek_offset_secs: Option<f64>) -> Vec<String> {
        let codec = self.get_codec_for_format(&self.format);
        let muxer = self.get_muxer_for_format(&self.format);

        let mut args = Vec::new();

        if !is_url_input(input) {
            if let Some(offset) = seek_offset_secs.filter(|offset| *offset > 0.0) {
                args.push("-ss".to_string());
                args.push(offset.to_string());
            }
        }

        args.extend([
            "-re".to_string(),
            "-i".to_string(),
            input.to_string(),
            "-f".to_string(),
            muxer.to_string(),
            "-acodec".to_string(),
            codec.to_string(),
            "-ab".to_string(),
            format!("{}k", self.bitrate),
            "-ar".to_string(),
            self.sample_rate.to_string(),
            "-ac".to_string(),
            self.channels.to_string(),
            "-loglevel".to_string(),
            "error".to_string(),
            "-".to_string(),
        ]);

        args
    }

    fn start_conversion_internal(
        &self,
        input: &str,
        seek_offset_secs: Option<f64>,
    ) -> Result<AudioProcess, Box<dyn std::error::Error + Send + Sync>> {
        debug!("Starting FFmpeg conversion for: {}", input);

        // Only check file existence for local files (not URLs)
        if !is_url_input(input) {
            let path = Path::new(input);
            if !path.exists() {
                return Err(format!("Input file does not exist: {}", input).into());
            }
        }

        let ffmpeg_args = self.build_ffmpeg_args(input, seek_offset_secs);

        let mut cmd = Command::new(&self.ffmpeg_path);
        cmd.args(&ffmpeg_args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        debug!("FFmpeg command: {:?}", cmd);

        let child = cmd.spawn()?;

        Ok(AudioProcess::new(child))
    }

    #[allow(dead_code)]
    pub fn start_streaming_service(
        self,
        legacy_track_queue_rx: Receiver<std::path::PathBuf>,
        listener_count: Arc<AtomicUsize>,
        listener_notify: Arc<Notify>,
        is_paused: Arc<AtomicBool>,
    ) -> Receiver<AudioChunk> {
        let (audio_tx, audio_rx) = unbounded::<AudioChunk>();

        tokio::spawn(async move {
            let mut current_process: Option<AudioProcess> = None;
            let mut current_track: Option<std::path::PathBuf> = None;
            let mut consecutive_failures: u32 = 0;
            let mut idle_since: Option<Instant> = None;

            loop {
                // Start new process if needed
                if current_process.is_none() {
                    let listener_count_val = listener_count.load(Ordering::SeqCst);

                    if listener_count_val == 0 {
                        if idle_since.is_none() {
                            idle_since = Some(Instant::now());
                            info!("No listeners, entering grace period");
                        } else if idle_since
                            .as_ref()
                            .map(|start| start.elapsed().as_secs() > IDLE_GRACE_PERIOD_SECS)
                            .unwrap_or(false)
                        {
                            is_paused.store(true, Ordering::SeqCst);
                            info!(
                                "No listeners for {}s, pausing FFmpeg processing",
                                IDLE_GRACE_PERIOD_SECS
                            );

                            tokio::select! {
                                _ = listener_notify.notified() => {
                                    is_paused.store(false, Ordering::SeqCst);
                                    info!("Listener connected, resuming FFmpeg processing");
                                    idle_since = None;
                                }
                            }

                            continue;
                        }
                    } else {
                        idle_since = None;
                    }

                    if listener_count_val > 0 {
                        is_paused.store(false, Ordering::SeqCst);

                        // Try to get next track
                        if let Ok(track) = legacy_track_queue_rx.try_recv() {
                            current_track = Some(track.clone());

                            // Check if track is a URL or local file
                            let track_str = track.to_str().unwrap_or("");
                            let result = if track_str.starts_with("http://")
                                || track_str.starts_with("https://")
                            {
                                info!("Starting stream from URL: {}", track_str);
                                self.start_conversion_from_url(track_str)
                            } else {
                                self.start_conversion_process(&track)
                            };

                            match result {
                                Ok(process) => {
                                    debug!("Started processing track: {:?}", track);
                                    current_process = Some(process);
                                }
                                Err(e) => {
                                    error!("Failed to start FFmpeg process for {:?}: {}", track, e);
                                    consecutive_failures += 1;
                                }
                            }
                        }
                    }
                }

                // Read from current process
                if let Some(ref mut process) = current_process {
                    match process.read_chunk() {
                        Ok(Some(chunk)) => {
                            let audio_chunk = AudioChunk { data: chunk };

                            if audio_tx.send(audio_chunk).is_err() {
                                warn!("Failed to send audio chunk - receiver dropped");
                                break;
                            }
                            consecutive_failures = 0;
                        }
                        Ok(None) => {
                            // Process finished successfully
                            info!("Track completed: {:?}", current_track);
                            current_process = None;
                            current_track = None;
                            consecutive_failures = 0;
                        }
                        Err(e) => {
                            let error_message = e.to_string();
                            if error_message.contains("non-zero status") {
                                debug!(
                                    "FFmpeg process failed for {:?}: {}",
                                    current_track, error_message
                                );
                            } else {
                                error!("Error reading from FFmpeg process: {}", error_message);
                            }
                            current_process = None;
                            current_track = None;
                            consecutive_failures += 1;
                        }
                    }
                }

                // Small delay to avoid busy waiting
                let delay_ms = if consecutive_failures > 0 {
                    calculate_backoff_ms(consecutive_failures)
                } else {
                    PROCESS_POLL_INTERVAL_MS
                };
                tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
            }
        });

        audio_rx
    }

    #[allow(dead_code)]
    pub fn start_timeline_streaming_service(
        self,
        mut timeline_rx: watch::Receiver<TimelineSnapshot>,
        chunk_tx: Sender<AudioChunk>,
        is_paused: Arc<AtomicBool>,
    ) {
        tokio::spawn(async move {
            let mut current_process: Option<AudioProcess> = None;
            let mut current_snapshot = timeline_rx.borrow().clone();
            let mut consecutive_failures: u32 = 0;

            let apply_snapshot_update =
                |next_snapshot: TimelineSnapshot,
                 current_snapshot: &mut TimelineSnapshot,
                 current_process: &mut Option<AudioProcess>,
                 consecutive_failures: &mut u32| {
                    let should_restart = next_snapshot.generation != current_snapshot.generation
                        || next_snapshot.track_path != current_snapshot.track_path;

                    if should_restart {
                        if let Some(process) = current_process.take() {
                            process.terminate()
                        }
                        // Reset consecutive failures on generation change
                        if next_snapshot.generation != current_snapshot.generation {
                            *consecutive_failures = 0;
                        }
                    }

                    *current_snapshot = next_snapshot;
                };

            loop {
                {
                    let next_snapshot = timeline_rx.borrow_and_update().clone();
                    let generation_changed = next_snapshot.generation != current_snapshot.generation;
                    let track_changed = next_snapshot.track_path != current_snapshot.track_path;
                    if generation_changed || track_changed {
                        apply_snapshot_update(
                            next_snapshot,
                            &mut current_snapshot,
                            &mut current_process,
                            &mut consecutive_failures,
                        );
                    }
                }

                is_paused.store(false, Ordering::SeqCst);

                if current_process.is_none() {
                    let track = current_snapshot.track_path.clone();

                    if track.as_os_str().is_empty() {
                        tokio::time::sleep(tokio::time::Duration::from_millis(
                            PROCESS_POLL_INTERVAL_MS,
                        ))
                        .await;
                        continue;
                    }

                    let track_str = track.to_string_lossy().to_string();
                    let result = self.start_conversion_with_seek(
                        &track_str,
                        Some(current_snapshot.elapsed_in_track_secs),
                    );

                    match result {
                        Ok(process) => {
                            debug!(
                                "Started timeline processing track: {:?}, generation: {}, seek: {}",
                                track,
                                current_snapshot.generation,
                                current_snapshot.elapsed_in_track_secs
                            );
                            current_process = Some(process);
                        }
                        Err(e) => {
                            error!(
                                "Failed to start FFmpeg process for timeline track {:?}: {}",
                                track, e
                            );
                            consecutive_failures += 1;
                        }
                    }
                }

                if let Some(process) = current_process.take() {
                    match tokio::task::spawn_blocking(move || {
                        let mut process = process;
                        let result = process.read_chunk();
                        (process, result)
                    })
                    .await
                    {
                        Ok((process, Ok(Some(chunk)))) => {
                            current_process = Some(process);
                            let audio_chunk = AudioChunk { data: chunk };

                            if chunk_tx.send(audio_chunk).is_err() {
                                warn!("Failed to send audio chunk - receiver dropped");
                                break;
                            }
                            consecutive_failures = 0;
                        }
                        Ok((_process, Ok(None))) => {
                            info!(
                                "Timeline track completed: {:?}, generation: {}",
                                current_snapshot.track_path, current_snapshot.generation
                            );
                            consecutive_failures = 0;
                        }
                        Ok((_process, Err(e))) => {
                            let error_message = e.to_string();
                            if error_message.contains("non-zero status") {
                                error!(
                                    "FFmpeg process failed for timeline track {:?}: {}",
                                    current_snapshot.track_path, error_message
                                );
                            } else {
                                error!("Error reading from FFmpeg process: {}", error_message);
                            }
                            consecutive_failures += 1;
                        }
                        Err(e) => {
                            error!("spawn_blocking panicked reading audio chunk: {}", e);
                            current_process = None;
                            consecutive_failures += 1;
                        }
                    }
                }

                let delay_ms = if consecutive_failures > 0 {
                    calculate_backoff_ms(consecutive_failures)
                } else {
                    PROCESS_POLL_INTERVAL_MS
                };

                if consecutive_failures > 0 {
                    tokio::select! {
                        _ = tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)) => {}
                        changed = timeline_rx.changed() => {
                            match changed {
                                Ok(()) => {
                                    let next_snapshot = timeline_rx.borrow_and_update().clone();
                                    apply_snapshot_update(
                                        next_snapshot,
                                        &mut current_snapshot,
                                        &mut current_process,
                                        &mut consecutive_failures,
                                    );
                                }
                                Err(_) => {
                                    debug!("Timeline sender dropped, stopping timeline streaming service");
                                    break;
                                }
                            }
                        }
                    }
                } else {
                    tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                }
            }

            if let Some(process) = current_process.take() {
                process.terminate();
            }
            is_paused.store(false, Ordering::SeqCst);
        });
    }
}

pub struct AudioProcess {
    child: Child,
    reader: Option<BufReader<std::process::ChildStdout>>,
    stderr: Option<BufReader<std::process::ChildStderr>>,
}

impl AudioProcess {
    fn new(mut child: Child) -> Self {
        let reader = child.stdout.take().map(BufReader::new);
        let stderr = child.stderr.take().map(BufReader::new);
        Self {
            child,
            reader,
            stderr,
        }
    }

    pub fn read_chunk(
        &mut self,
    ) -> Result<Option<Bytes>, Box<dyn std::error::Error + Send + Sync>> {
        if let Some(ref mut reader) = self.reader {
            let mut buffer = [0u8; AUDIO_CHUNK_SIZE];

            match reader.read(&mut buffer) {
                Ok(0) => {
                    // EOF reached
                    let success = self.wait_for_completion()?;
                    if success {
                        Ok(None)
                    } else {
                        Err("FFmpeg process exited with non-zero status".into())
                    }
                }
                Ok(bytes_read) => Ok(Some(Bytes::copy_from_slice(&buffer[..bytes_read]))),
                Err(e) => {
                    error!("Error reading from FFmpeg stdout: {}", e);
                    Err(e.into())
                }
            }
        } else {
            Err("No stdout reader available".into())
        }
    }

    fn wait_for_completion(&mut self) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let stderr_output = if let Some(ref mut stderr) = self.stderr {
            let mut buf = Vec::with_capacity(4096);
            let _ = stderr.by_ref().take(4096).read_to_end(&mut buf);
            String::from_utf8_lossy(&buf).to_string()
        } else {
            String::new()
        };

        match self.child.wait() {
            Ok(status) => {
                if status.success() {
                    debug!("FFmpeg process completed successfully");
                    Ok(true)
                } else {
                    warn!(
                        "FFmpeg process exited with status: {} - stderr: {}",
                        status,
                        stderr_output.trim()
                    );
                    Ok(false)
                }
            }
            Err(e) => {
                error!("Error waiting for FFmpeg process: {}", e);
                Err(e.into())
            }
        }
    }

    #[allow(dead_code)]
    fn terminate(mut self) {
        if let Err(e) = self.child.kill() {
            debug!("Unable to kill FFmpeg process cleanly: {}", e);
        }
        if let Err(e) = self.child.wait() {
            debug!("Unable to wait for killed FFmpeg process: {}", e);
        }
    }
}
#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub data: Bytes,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_metadata::TrackMetadata;
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio::sync::watch;

    fn snapshot_with(track: &str, generation: u64) -> TimelineSnapshot {
        TimelineSnapshot {
            track_path: PathBuf::from(track),
            track_index: 0,
            elapsed_in_track_secs: 0.0,
            generation,
            current_metadata: TrackMetadata::default(),
        }
    }

    fn create_fake_ffmpeg_script(script_body: &str) -> String {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().expect("temporary directory should be created");
        let path = dir.path().join("fake-ffmpeg.sh");

        fs::write(&path, script_body).expect("fake ffmpeg script should be written");

        let mut permissions = fs::metadata(&path)
            .expect("fake ffmpeg script metadata should be readable")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions)
            .expect("fake ffmpeg script should be marked executable");

        let script_path = path.to_string_lossy().to_string();
        std::mem::forget(dir);
        script_path
    }

    #[test]
    fn given_mp3_format_when_getting_codec_then_returns_libmp3lame() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "mp3".to_string());
        assert_eq!(processor.get_codec_for_format("mp3"), "libmp3lame");
    }

    #[test]
    fn given_opus_format_when_getting_codec_then_returns_libopus() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "opus".to_string());
        assert_eq!(processor.get_codec_for_format("opus"), "libopus");
    }

    #[test]
    fn given_aac_format_when_getting_codec_then_returns_aac() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "aac".to_string());
        assert_eq!(processor.get_codec_for_format("aac"), "aac");
    }

    #[test]
    fn given_vorbis_format_when_getting_codec_then_returns_libvorbis() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "vorbis".to_string());
        assert_eq!(processor.get_codec_for_format("vorbis"), "libvorbis");
    }

    #[test]
    fn given_ogg_format_when_getting_codec_then_returns_libvorbis() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "ogg".to_string());
        assert_eq!(processor.get_codec_for_format("ogg"), "libvorbis");
    }

    #[test]
    fn given_flac_format_when_getting_codec_then_returns_flac() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "flac".to_string());
        assert_eq!(processor.get_codec_for_format("flac"), "flac");
    }

    #[test]
    fn given_unknown_format_when_getting_codec_then_returns_default_libmp3lame() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "unknown".to_string());
        assert_eq!(processor.get_codec_for_format("unknown"), "libmp3lame");
    }

    #[test]
    fn given_aac_format_when_getting_muxer_then_returns_adts() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "aac".to_string());
        assert_eq!(processor.get_muxer_for_format("aac"), "adts");
    }

    #[test]
    fn given_mp3_format_when_getting_muxer_then_returns_mp3() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "mp3".to_string());
        assert_eq!(processor.get_muxer_for_format("mp3"), "mp3");
    }

    #[test]
    fn given_opus_format_when_getting_muxer_then_returns_opus() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "opus".to_string());
        assert_eq!(processor.get_muxer_for_format("opus"), "opus");
    }

    #[test]
    fn given_vorbis_format_when_getting_muxer_then_returns_ogg() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "vorbis".to_string());
        assert_eq!(processor.get_muxer_for_format("vorbis"), "ogg");
    }

    #[test]
    fn given_ogg_format_when_getting_muxer_then_returns_ogg() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "ogg".to_string());
        assert_eq!(processor.get_muxer_for_format("ogg"), "ogg");
    }

    #[test]
    fn given_flac_format_when_getting_muxer_then_returns_flac() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "flac".to_string());
        assert_eq!(processor.get_muxer_for_format("flac"), "flac");
    }

    #[test]
    fn given_unknown_format_when_getting_muxer_then_returns_default_mp3() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "unknown".to_string());
        assert_eq!(processor.get_muxer_for_format("unknown"), "mp3");
    }
    #[test]
    fn given_zero_failures_when_calculating_backoff_then_returns_initial_backoff_ms() {
        assert_eq!(calculate_backoff_ms(0), 1000);
    }

    #[test]
    fn given_one_failure_when_calculating_backoff_then_returns_doubled_initial_backoff_ms() {
        assert_eq!(calculate_backoff_ms(1), 2000);
    }

    #[test]
    fn given_many_failures_when_calculating_backoff_then_returns_max_backoff_ms() {
        assert_eq!(calculate_backoff_ms(100), 5_000);
    }

    #[test]
    fn given_max_u32_failures_when_calculating_backoff_then_returns_max_backoff_ms() {
        assert_eq!(calculate_backoff_ms(u32::MAX), 5_000);
    }

    #[test]
    fn given_missing_config_path_when_resolving_then_uses_env_command_name() {
        let resolved = resolve_ffmpeg_path(None, Some("ffmpeg-custom".to_string()));
        assert_eq!(resolved, "ffmpeg-custom");
    }

    #[test]
    fn given_missing_config_and_env_when_resolving_then_uses_command_name_fallback() {
        let resolved = resolve_ffmpeg_path(None, None);
        let expected = if PathBuf::from("/ffmpeg").exists() {
            "/ffmpeg"
        } else {
            "ffmpeg"
        };
        assert_eq!(resolved, expected);
    }

    #[test]
    fn given_idle_grace_period_constant_then_equals_60_seconds() {
        assert_eq!(IDLE_GRACE_PERIOD_SECS, 60);
    }

    #[test]
    fn given_local_input_with_seek_when_building_ffmpeg_args_then_places_ss_before_re_and_i() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "mp3".to_string());
        let args = processor.build_ffmpeg_args("/tmp/song.mp3", Some(12.5));

        let ss_index = args
            .iter()
            .position(|arg| arg == "-ss")
            .expect("-ss must exist for local files with seek");
        let re_index = args
            .iter()
            .position(|arg| arg == "-re")
            .expect("-re must exist");
        let i_index = args
            .iter()
            .position(|arg| arg == "-i")
            .expect("-i must exist");

        assert!(ss_index < re_index);
        assert!(re_index < i_index);
        assert_eq!(args[ss_index + 1], "12.5");
    }

    #[test]
    fn given_url_input_with_seek_when_building_ffmpeg_args_then_omits_ss_and_keeps_re_before_i() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "mp3".to_string());
        let args = processor.build_ffmpeg_args("https://example.com/stream.mp3", Some(42.0));

        assert!(!args.iter().any(|arg| arg == "-ss"));

        let re_index = args
            .iter()
            .position(|arg| arg == "-re")
            .expect("-re must exist");
        let i_index = args
            .iter()
            .position(|arg| arg == "-i")
            .expect("-i must exist");
        assert!(re_index < i_index);
    }

    #[test]
    fn given_local_input_without_seek_when_building_ffmpeg_args_then_omits_ss() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "mp3".to_string());
        let args = processor.build_ffmpeg_args("/tmp/song.mp3", None);
        assert!(!args.iter().any(|arg| arg == "-ss"));
    }

    #[test]
    fn given_zero_or_negative_seek_when_building_ffmpeg_args_then_omits_ss() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "mp3".to_string());
        let args_zero = processor.build_ffmpeg_args("/tmp/song.mp3", Some(0.0));
        let args_negative = processor.build_ffmpeg_args("/tmp/song.mp3", Some(-1.0));

        assert!(!args_zero.iter().any(|arg| arg == "-ss"));
        assert!(!args_negative.iter().any(|arg| arg == "-ss"));
    }

    #[test]
    fn given_http_and_https_inputs_when_checking_is_url_then_detects_both_protocols() {
        assert!(is_url_input("http://example.com/stream.mp3"));
        assert!(is_url_input("https://example.com/stream.mp3"));
        assert!(!is_url_input(
            Path::new("/tmp/song.mp3").to_str().unwrap_or_default()
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn given_snapshot_without_encoding_active_flag_when_processing_then_ffmpeg_decision_not_gated(
    ) {
        let fake_ffmpeg_path = create_fake_ffmpeg_script(
            r#"#!/bin/sh
printf 'audio'
exit 0
"#,
        );

        let processor =
            FFmpegProcessor::new(Some(fake_ffmpeg_path), 48000, 192, 2, "mp3".to_string());
        let (timeline_tx, timeline_rx) =
            watch::channel(snapshot_with("https://example.com/ungated-stream", 0));
        let (chunk_tx, chunk_rx) = unbounded::<AudioChunk>();
        let is_paused = Arc::new(AtomicBool::new(false));

        processor.start_timeline_streaming_service(timeline_rx, chunk_tx, is_paused);

        let recv_result =
            tokio::task::spawn_blocking(move || chunk_rx.recv_timeout(Duration::from_millis(300)))
                .await
                .expect("blocking receiver task should join successfully");
        drop(timeline_tx);

        assert!(
            recv_result.is_ok(),
            "expected FFmpeg spawn/streaming decision to be independent from is_encoding_active"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn given_ffmpeg_fails_when_generation_changes_then_consecutive_failures_resets() {
        let fake_ffmpeg_path = create_fake_ffmpeg_script(
            r#"#!/bin/sh
for arg in "$@"; do
  case "$arg" in
    *ok-stream*)
      printf 'audio'
      exit 0
      ;;
  esac
done
exit 1
"#,
        );

        let processor =
            FFmpegProcessor::new(Some(fake_ffmpeg_path), 48000, 192, 2, "mp3".to_string());
        let (timeline_tx, timeline_rx) =
            watch::channel(snapshot_with("https://example.com/failing-stream", 0));
        let (chunk_tx, chunk_rx) = unbounded::<AudioChunk>();
        let is_paused = Arc::new(AtomicBool::new(false));

        processor.start_timeline_streaming_service(timeline_rx, chunk_tx, is_paused);

        tokio::time::sleep(Duration::from_millis(100)).await;

        timeline_tx
            .send(snapshot_with("", 1))
            .expect("first generation change should be delivered");

        tokio::time::sleep(Duration::from_millis(30)).await;

        timeline_tx
            .send(snapshot_with("https://example.com/ok-stream", 2))
            .expect("second generation change should be delivered");

        let recv_result =
            tokio::task::spawn_blocking(move || chunk_rx.recv_timeout(Duration::from_millis(350)))
                .await
                .expect("blocking receiver task should join successfully");
        drop(timeline_tx);

        assert!(
            recv_result.is_ok(),
            "expected generation change to reset failure backoff so next track starts quickly"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn given_is_encoding_active_false_when_processing_then_is_paused_never_true() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "mp3".to_string());
        let (timeline_tx, timeline_rx) =
            watch::channel(snapshot_with("https://example.com/stream", 0));
        let (chunk_tx, _chunk_rx) = unbounded::<AudioChunk>();
        let is_paused = Arc::new(AtomicBool::new(false));

        processor.start_timeline_streaming_service(timeline_rx, chunk_tx, Arc::clone(&is_paused));

        tokio::time::sleep(Duration::from_millis(80)).await;
        drop(timeline_tx);

        assert!(
            !is_paused.load(Ordering::SeqCst),
            "expected audio processor to never enter paused state based on is_encoding_active"
        );
    }
}
