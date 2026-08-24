use bytes::Bytes;
#[cfg(test)]
use crossbeam_channel::unbounded;
use crossbeam_channel::Sender;
use log::{debug, error, info, warn};
use std::io::ErrorKind;
use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{watch, Notify};

use crate::audio_metadata::TrackMetadata;
use crate::playback_director::TimelineSnapshot;

// Constants for audio processing configuration
const AUDIO_CHUNK_SIZE: usize = 8192; // 8KB chunks for reading audio data
const PROCESS_POLL_INTERVAL_MS: u64 = 10; // How often to poll FFmpeg process
const INITIAL_BACKOFF_MS: u64 = 1000;
const MAX_BACKOFF_MS: u64 = 5_000;
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

    fn build_ffmpeg_args(
        &self,
        input: &str,
        seek_offset_secs: Option<f64>,
        metadata: Option<&TrackMetadata>,
    ) -> Vec<String> {
        let codec = self.get_codec_for_format(&self.format);
        let muxer = self.get_muxer_for_format(&self.format);

        let mut args = Vec::new();

        if !is_url_input(input) {
            if let Some(offset) = seek_offset_secs.filter(|offset| *offset > 0.0) {
                args.push("-ss".to_string());
                args.push(offset.to_string());
            }
        }

        args.extend(["-re".to_string(), "-i".to_string(), input.to_string()]);

        // Pass metadata as codec tags for OGG/FLAC container formats
        // so Vorbis/Opus/FLAC comment headers carry track info
        if let Some(meta) = metadata {
            let fmt = self.format.to_lowercase();
            if fmt == "ogg" || fmt == "opus" || fmt == "vorbis" || fmt == "flac" {
                args.push("-metadata".to_string());
                args.push(format!("title={}", meta.title));
                args.push("-metadata".to_string());
                args.push(format!("artist={}", meta.artist));
            }
        }

        args.extend([
            "-vn".to_string(),
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
        metadata: Option<&TrackMetadata>,
    ) -> Result<AudioProcess, Box<dyn std::error::Error + Send + Sync>> {
        debug!("Starting FFmpeg conversion for: {}", input);

        // Only check file existence for local files (not URLs)
        if !is_url_input(input) {
            let path = Path::new(input);
            if !path.exists() {
                return Err(format!("Input file does not exist: {}", input).into());
            }
        }

        let ffmpeg_args = self.build_ffmpeg_args(input, seek_offset_secs, metadata);

        let mut cmd = Command::new(&self.ffmpeg_path);
        cmd.args(&ffmpeg_args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        debug!("FFmpeg command: {:?}", cmd);

        let child = cmd.spawn()?;

        Ok(AudioProcess::new(child))
    }

    pub fn start_timeline_streaming_service(
        self,
        mut timeline_rx: watch::Receiver<TimelineSnapshot>,
        chunk_tx: Sender<AudioChunk>,
        is_paused: Arc<AtomicBool>,
        listener_count: Arc<AtomicUsize>,
        listener_notify: Arc<Notify>,
    ) {
        tokio::spawn(async move {
            let mut current_process: Option<AudioProcess> = None;
            let mut current_snapshot = timeline_rx.borrow().clone();
            let mut consecutive_failures: u32 = 0;
            let mut idle_since: Option<Instant> = None;

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
                // Idle detection: pause FFmpeg when no listeners
                if listener_count.load(Ordering::SeqCst) == 0 {
                    if idle_since.is_none() {
                        idle_since = Some(Instant::now());
                        info!("No listeners, entering grace period");
                    } else if idle_since
                        .as_ref()
                        .map(|start| start.elapsed().as_secs() > IDLE_GRACE_PERIOD_SECS)
                        .unwrap_or(false)
                    {
                        if let Some(process) = current_process.take() {
                            process.terminate();
                        }
                        is_paused.store(true, Ordering::SeqCst);
                        info!(
                            "No listeners for {}s, pausing FFmpeg processing",
                            IDLE_GRACE_PERIOD_SECS
                        );

                        loop {
                            tokio::select! {
                                _ = listener_notify.notified() => {
                                    info!("Listener connected, resuming FFmpeg processing");
                                    break;
                                }
                                result = timeline_rx.changed() => {
                                    match result {
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
                                            debug!("Timeline sender dropped during idle pause, stopping timeline streaming service");
                                            if let Some(process) = current_process.take() {
                                                process.terminate();
                                            }
                                            is_paused.store(false, Ordering::SeqCst);
                                            return;
                                        }
                                    }
                                }
                            }
                        }

                        is_paused.store(false, Ordering::SeqCst);
                        idle_since = None;
                        continue;
                    }
                } else {
                    idle_since = None;
                }

                {
                    let next_snapshot = timeline_rx.borrow_and_update().clone();
                    let generation_changed =
                        next_snapshot.generation != current_snapshot.generation;
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
                    let result = self.start_conversion_internal(
                        &track_str,
                        Some(current_snapshot.elapsed_in_track_secs),
                        Some(&current_snapshot.current_metadata),
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

    fn create_fake_ffmpeg_script(script_body: &str) -> (tempfile::TempDir, String) {
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
        (dir, script_path)
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
        let args = processor.build_ffmpeg_args("/tmp/song.mp3", Some(12.5), None);

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
        let args = processor.build_ffmpeg_args("https://example.com/stream.mp3", Some(42.0), None);

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
        let args = processor.build_ffmpeg_args("/tmp/song.mp3", None, None);
        assert!(!args.iter().any(|arg| arg == "-ss"));
    }

    #[test]
    fn given_zero_or_negative_seek_when_building_ffmpeg_args_then_omits_ss() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "mp3".to_string());
        let args_zero = processor.build_ffmpeg_args("/tmp/song.mp3", Some(0.0), None);
        let args_negative = processor.build_ffmpeg_args("/tmp/song.mp3", Some(-1.0), None);

        assert!(!args_zero.iter().any(|arg| arg == "-ss"));
        assert!(!args_negative.iter().any(|arg| arg == "-ss"));
    }

    #[test]
    fn given_audio_conversion_when_building_ffmpeg_args_then_disables_video_streams() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "mp3".to_string());
        let args = processor.build_ffmpeg_args("/tmp/song.flac", None, None);
        assert!(args.iter().any(|arg| arg == "-vn"));
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
    async fn given_timeline_snapshot_when_processing_then_ffmpeg_streams_chunks() {
        let (_temp_dir, fake_ffmpeg_path) = create_fake_ffmpeg_script(
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
        let listener_count = Arc::new(AtomicUsize::new(0));
        let listener_notify = Arc::new(Notify::new());

        processor.start_timeline_streaming_service(
            timeline_rx,
            chunk_tx,
            is_paused,
            listener_count,
            listener_notify,
        );

        let recv_result =
            tokio::task::spawn_blocking(move || chunk_rx.recv_timeout(Duration::from_millis(300)))
                .await
                .expect("blocking receiver task should join successfully");
        drop(timeline_tx);

        assert!(
            recv_result.is_ok(),
            "expected timeline processing to stream audio chunks"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn given_ffmpeg_fails_when_generation_changes_then_consecutive_failures_resets() {
        let (_temp_dir, fake_ffmpeg_path) = create_fake_ffmpeg_script(
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
        let listener_count = Arc::new(AtomicUsize::new(0));
        let listener_notify = Arc::new(Notify::new());

        processor.start_timeline_streaming_service(
            timeline_rx,
            chunk_tx,
            is_paused,
            listener_count,
            listener_notify,
        );

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
    async fn given_no_listeners_when_processing_then_is_paused_stays_false() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "mp3".to_string());
        let (timeline_tx, timeline_rx) =
            watch::channel(snapshot_with("https://example.com/stream", 0));
        let (chunk_tx, _chunk_rx) = unbounded::<AudioChunk>();
        let is_paused = Arc::new(AtomicBool::new(false));
        let listener_count = Arc::new(AtomicUsize::new(0));
        let listener_notify = Arc::new(Notify::new());

        processor.start_timeline_streaming_service(
            timeline_rx,
            chunk_tx,
            Arc::clone(&is_paused),
            listener_count,
            listener_notify,
        );

        tokio::time::sleep(Duration::from_millis(80)).await;
        drop(timeline_tx);

        assert!(
            !is_paused.load(Ordering::SeqCst),
            "expected is_paused to stay false when no listeners are connected"
        );
    }

    // --- FFmpeg metadata passthrough tests ---

    fn test_metadata() -> TrackMetadata {
        TrackMetadata {
            title: "Test Song".to_string(),
            artist: "Test Artist".to_string(),
            album: "Test Album".to_string(),
            file_path: "/tmp/test.mp3".to_string(),
        }
    }

    #[test]
    fn given_ogg_format_with_metadata_when_building_args_then_includes_metadata_tags() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "ogg".to_string());
        let metadata = test_metadata();
        let args = processor.build_ffmpeg_args("/tmp/song.flac", None, Some(&metadata));

        // Should contain -metadata title=... and -metadata artist=...
        let meta_indices: Vec<usize> = args
            .iter()
            .enumerate()
            .filter(|(_, arg)| *arg == "-metadata")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(meta_indices.len(), 2, "expected two -metadata args for OGG");
        assert_eq!(args[meta_indices[0] + 1], "title=Test Song");
        assert_eq!(args[meta_indices[1] + 1], "artist=Test Artist");
        // Metadata should appear before the codec/muxer args
        assert!(meta_indices[0] < args.iter().position(|a| a == "-acodec").unwrap());
    }

    #[test]
    fn given_flac_format_with_metadata_when_building_args_then_includes_metadata_tags() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "flac".to_string());
        let metadata = test_metadata();
        let args = processor.build_ffmpeg_args("/tmp/song.flac", None, Some(&metadata));

        let meta_count = args.iter().filter(|a| *a == "-metadata").count();
        assert_eq!(meta_count, 2, "expected two -metadata args for FLAC");
    }

    #[test]
    fn given_mp3_format_with_metadata_when_building_args_then_excludes_metadata_tags() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "mp3".to_string());
        let metadata = test_metadata();
        let args = processor.build_ffmpeg_args("/tmp/song.mp3", None, Some(&metadata));

        assert!(
            !args.iter().any(|a| a == "-metadata"),
            "MP3 format should not include -metadata args"
        );
    }

    #[test]
    fn given_aac_format_with_metadata_when_building_args_then_excludes_metadata_tags() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "aac".to_string());
        let metadata = test_metadata();
        let args = processor.build_ffmpeg_args("/tmp/song.aac", None, Some(&metadata));

        assert!(
            !args.iter().any(|a| a == "-metadata"),
            "AAC format should not include -metadata args"
        );
    }

    #[test]
    fn given_ogg_format_without_metadata_when_building_args_then_excludes_metadata_tags() {
        let processor = FFmpegProcessor::new(None, 48000, 192, 2, "ogg".to_string());
        let args = processor.build_ffmpeg_args("/tmp/song.flac", None, None);

        assert!(
            !args.iter().any(|a| a == "-metadata"),
            "No -metadata args when metadata is None"
        );
    }
    // --- Idle detection tests ---

    #[tokio::test(flavor = "current_thread")]
    async fn given_listeners_present_when_processing_then_idle_detection_is_noop() {
        // With listener_count > 0, idle detection should never engage.
        let (_temp_dir, fake_ffmpeg_path) = create_fake_ffmpeg_script(
            r#"#!/bin/sh
printf 'audio'
exit 0
"#,
        );

        let processor =
            FFmpegProcessor::new(Some(fake_ffmpeg_path), 48000, 192, 2, "mp3".to_string());
        let (timeline_tx, timeline_rx) =
            watch::channel(snapshot_with("https://example.com/active-stream", 0));
        let (chunk_tx, chunk_rx) = unbounded::<AudioChunk>();
        let is_paused = Arc::new(AtomicBool::new(false));
        let listener_count = Arc::new(AtomicUsize::new(1)); // Listeners present
        let listener_notify = Arc::new(Notify::new());

        processor.start_timeline_streaming_service(
            timeline_rx,
            chunk_tx,
            Arc::clone(&is_paused),
            listener_count,
            listener_notify,
        );

        // Use try_recv loop instead of spawn_blocking so the current_thread runtime
        // can poll the streaming task between attempts.
        let recv_result = loop {
            match chunk_rx.try_recv() {
                Ok(chunk) => break Ok(chunk),
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    break Err(());
                }
            }
        };
        drop(timeline_tx);

        assert!(
            recv_result.is_ok(),
            "expected audio to stream normally when listeners are present"
        );
        assert!(
            !is_paused.load(Ordering::SeqCst),
            "is_paused should never become true when listeners are present"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn given_no_listeners_when_grace_period_not_expired_then_is_paused_stays_false() {
        // With listener_count = 0 but grace period not expired, is_paused stays false.
        let (_temp_dir, fake_ffmpeg_path) = create_fake_ffmpeg_script(
            r#"#!/bin/sh
printf 'audio'
exit 0
"#,
        );

        let processor =
            FFmpegProcessor::new(Some(fake_ffmpeg_path), 48000, 192, 2, "mp3".to_string());
        let (timeline_tx, timeline_rx) =
            watch::channel(snapshot_with("https://example.com/idle-grace-stream", 0));
        let (chunk_tx, _chunk_rx) = unbounded::<AudioChunk>();
        let is_paused = Arc::new(AtomicBool::new(false));
        let listener_count = Arc::new(AtomicUsize::new(0)); // No listeners
        let listener_notify = Arc::new(Notify::new());

        processor.start_timeline_streaming_service(
            timeline_rx,
            chunk_tx,
            Arc::clone(&is_paused),
            listener_count,
            listener_notify,
        );

        // Wait well under the 60s grace period
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(timeline_tx);

        assert!(
            !is_paused.load(Ordering::SeqCst),
            "is_paused should stay false before grace period expires"
        );
    }

    #[test]
    fn given_absolute_path_when_checking_looks_like_filesystem_path_then_returns_true() {
        assert!(looks_like_filesystem_path("/usr/bin/ffmpeg"));
    }

    #[test]
    fn given_relative_path_when_checking_looks_like_filesystem_path_then_returns_true() {
        assert!(looks_like_filesystem_path("./ffmpeg"));
        assert!(looks_like_filesystem_path("bin/ffmpeg"));
    }

    #[test]
    fn given_windows_style_path_with_forward_slash_when_checking_looks_like_filesystem_path_then_returns_true(
    ) {
        assert!(looks_like_filesystem_path("C:/tools/ffmpeg.exe"));
    }

    #[test]
    fn given_url_string_when_checking_looks_like_filesystem_path_then_returns_true() {
        assert!(
            looks_like_filesystem_path("http://example.com/stream.mp3"),
            "current implementation matches on any '/' so URLs also count as filesystem paths"
        );
    }

    #[test]
    fn given_plain_word_when_checking_looks_like_filesystem_path_then_returns_false() {
        assert!(!looks_like_filesystem_path("ffmpeg"));
    }

    #[test]
    fn given_bare_executable_name_with_extension_when_checking_looks_like_filesystem_path_then_returns_false(
    ) {
        assert!(!looks_like_filesystem_path("ffmpeg.exe"));
    }

    #[test]
    fn given_backslash_only_path_when_checking_looks_like_filesystem_path_then_returns_false() {
        assert!(!looks_like_filesystem_path(r"C:\tools\ffmpeg.exe"));
    }

    #[test]
    fn given_empty_string_when_checking_looks_like_filesystem_path_then_returns_false() {
        assert!(!looks_like_filesystem_path(""));
    }

    #[test]
    fn given_http_and_https_inputs_when_pinning_is_url_input_edge_cases_then_matches_current_truth_table(
    ) {
        assert!(is_url_input("https://example.com/stream.mp3"));
        assert!(!is_url_input(""));
        // Case-sensitive prefix check: uppercase scheme is not detected
        assert!(!is_url_input("HTTP://example.com/stream.mp3"));
        assert!(!is_url_input("HTTPS://example.com/stream.mp3"));
        // Only http(s) schemes count; other URL schemes fall through
        assert!(!is_url_input("ftp://example.com/stream.mp3"));
        assert!(!is_url_input("file:///tmp/song.mp3"));
    }

    // --- AudioProcess::read_chunk / wait_for_completion tests ---

    /// Linux can transiently fail `execve` with `ETXTBSY` when many tests spawn
    /// freshly written scripts concurrently; retry briefly before giving up.
    fn retry_while_text_file_busy<T, E: std::fmt::Display>(
        mut attempt: impl FnMut() -> Result<T, E>,
    ) -> Result<T, E> {
        const MAX_ATTEMPTS: usize = 100;
        let mut last_err: Option<E> = None;
        for _ in 0..MAX_ATTEMPTS {
            match attempt() {
                Ok(value) => return Ok(value),
                Err(err) => {
                    if !err.to_string().contains("Text file busy") {
                        return Err(err);
                    }
                    last_err = Some(err);
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
        panic!(
            "script still busy after {MAX_ATTEMPTS} attempts; last error: {}",
            last_err.expect("a busy error must have been recorded")
        );
    }

    fn spawn_audio_process_from_script(script_path: &str) -> AudioProcess {
        let mut cmd = Command::new(script_path);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = retry_while_text_file_busy(|| cmd.spawn().map_err(Box::<std::io::Error>::new))
            .expect("fake ffmpeg process should spawn");
        AudioProcess::new(child)
    }

    #[test]
    fn given_audio_output_and_success_exit_when_reading_chunks_then_returns_bytes_before_eof_none()
    {
        let (_temp_dir, fake_ffmpeg_path) = create_fake_ffmpeg_script(
            r#"#!/bin/sh
printf 'hello-audio'
exit 0
"#,
        );
        let mut process = spawn_audio_process_from_script(&fake_ffmpeg_path);

        let first = process.read_chunk().expect("first read should succeed");
        let chunk = first.expect("expected buffered audio bytes before EOF");
        assert_eq!(&chunk[..], b"hello-audio");

        let eof = process.read_chunk().expect("EOF read should not error");
        assert!(
            eof.is_none(),
            "expected None once the successful child reaches EOF"
        );
    }

    #[test]
    fn given_silent_success_exit_when_reading_chunk_then_returns_eof_none() {
        let (_temp_dir, fake_ffmpeg_path) = create_fake_ffmpeg_script(
            r#"#!/bin/sh
exit 0
"#,
        );
        let mut process = spawn_audio_process_from_script(&fake_ffmpeg_path);

        let result = process.read_chunk().expect("EOF read should not error");
        assert!(result.is_none(), "immediate EOF with exit 0 yields None");
    }

    #[test]
    fn given_buffered_audio_with_nonzero_exit_when_reading_to_eof_then_yields_bytes_then_errors() {
        let (_temp_dir, fake_ffmpeg_path) = create_fake_ffmpeg_script(
            r#"#!/bin/sh
printf 'partial'
exit 3
"#,
        );
        let mut process = spawn_audio_process_from_script(&fake_ffmpeg_path);

        // Current behavior: buffered data is handed out before the failure surfaces.
        let chunk = process
            .read_chunk()
            .expect("buffered bytes are returned even though the child will fail");
        assert_eq!(&chunk.expect("expected buffered bytes")[..], b"partial");

        let err = process
            .read_chunk()
            .expect_err("EOF after non-zero exit must surface as an error")
            .to_string();
        assert!(
            err.contains("non-zero status"),
            "expected non-zero-status error, got: {err}"
        );
    }

    #[test]
    fn given_immediate_nonzero_exit_when_reading_chunk_then_errors_on_nonzero_status() {
        let (_temp_dir, fake_ffmpeg_path) = create_fake_ffmpeg_script(
            r#"#!/bin/sh
echo 'boom' >&2
exit 1
"#,
        );
        let mut process = spawn_audio_process_from_script(&fake_ffmpeg_path);

        let err = process
            .read_chunk()
            .expect_err("EOF after non-zero exit must surface as an error")
            .to_string();
        assert!(
            err.contains("non-zero status"),
            "expected non-zero-status error, got: {err}"
        );
    }

    #[test]
    fn given_zero_exit_when_waiting_for_completion_then_returns_ok_true() {
        let (_temp_dir, fake_ffmpeg_path) = create_fake_ffmpeg_script(
            r#"#!/bin/sh
exit 0
"#,
        );
        let mut process = spawn_audio_process_from_script(&fake_ffmpeg_path);

        let completed = process
            .wait_for_completion()
            .expect("wait should never error on a clean child");
        assert!(completed, "zero exit status should report success");
    }

    #[test]
    fn given_nonzero_exit_when_waiting_for_completion_then_returns_ok_false() {
        let (_temp_dir, fake_ffmpeg_path) = create_fake_ffmpeg_script(
            r#"#!/bin/sh
exit 9
"#,
        );
        let mut process = spawn_audio_process_from_script(&fake_ffmpeg_path);

        let completed = process
            .wait_for_completion()
            .expect("wait should report failure as Ok(false), not Err");
        assert!(!completed, "non-zero exit status should report failure");
    }

    // --- FFmpegProcessor::check_ffmpeg_available tests ---

    #[test]
    fn given_version_printing_script_when_checking_availability_then_returns_ok() {
        let (_temp_dir, fake_ffmpeg_path) = create_fake_ffmpeg_script(
            r#"#!/bin/sh
echo 'ffmpeg version 7.0-fake'
exit 0
"#,
        );
        let processor =
            FFmpegProcessor::new(Some(fake_ffmpeg_path), 48000, 192, 2, "mp3".to_string());

        retry_while_text_file_busy(|| processor.check_ffmpeg_available())
            .expect("a zero-exit '-version' run should validate as available");
    }

    #[test]
    fn given_missing_ffmpeg_executable_when_checking_availability_then_reports_not_found() {
        // Constructed directly so resolve_ffmpeg_path cannot fall back to another binary.
        let processor = FFmpegProcessor {
            ffmpeg_path: "/nonexistent/funkstrom-missing-ffmpeg".to_string(),
            sample_rate: 48000,
            bitrate: 192,
            channels: 2,
            format: "mp3".to_string(),
        };

        let err = processor
            .check_ffmpeg_available()
            .expect_err("a missing executable must be reported");
        assert!(
            err.to_string().contains("was not found"),
            "expected not-found message, got: {err}"
        );
    }

    #[test]
    fn given_failing_version_script_when_checking_availability_then_reports_not_found_at_path() {
        let (_temp_dir, fake_ffmpeg_path) = create_fake_ffmpeg_script(
            r#"#!/bin/sh
exit 2
"#,
        );
        let processor = FFmpegProcessor::new(
            Some(fake_ffmpeg_path.clone()),
            48000,
            192,
            2,
            "mp3".to_string(),
        );

        let err = retry_while_text_file_busy(|| processor.check_ffmpeg_available())
            .expect_err("a failing '-version' run must be reported");
        assert!(
            err.to_string()
                .contains(&format!("FFmpeg not found at path: {fake_ffmpeg_path}")),
            "expected path-specific failure message, got: {err}"
        );
    }
}
