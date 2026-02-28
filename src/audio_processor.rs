use bytes::Bytes;
use crossbeam_channel::{unbounded, Receiver};
use log::{debug, error, info, warn};
use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};

// Constants for audio processing configuration
const AUDIO_CHUNK_SIZE: usize = 8192; // 8KB chunks for reading audio data
const PROCESS_POLL_INTERVAL_MS: u64 = 10; // How often to poll FFmpeg process
const INITIAL_BACKOFF_MS: u64 = 1000;
const MAX_BACKOFF_MS: u64 = 30_000;

fn calculate_backoff_ms(consecutive_failures: u32) -> u64 {
    let backoff =
        INITIAL_BACKOFF_MS.saturating_mul(1u64.wrapping_shl(consecutive_failures.min(15)));
    backoff.min(MAX_BACKOFF_MS)
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
        Self {
            ffmpeg_path: ffmpeg_path.unwrap_or_else(|| "ffmpeg".to_string()),
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

        let output = Command::new(&self.ffmpeg_path)
            .args(["-version"])
            .output()?;

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

    pub fn start_conversion_from_url(
        &self,
        url: &str,
    ) -> Result<AudioProcess, Box<dyn std::error::Error + Send + Sync>> {
        self.start_conversion(url)
    }

    fn start_conversion(
        &self,
        input: &str,
    ) -> Result<AudioProcess, Box<dyn std::error::Error + Send + Sync>> {
        debug!("Starting FFmpeg conversion for: {}", input);

        // Only check file existence for local files (not URLs)
        if !input.starts_with("http://") && !input.starts_with("https://") {
            let path = Path::new(input);
            if !path.exists() {
                return Err(format!("Input file does not exist: {}", input).into());
            }
        }

        let codec = self.get_codec_for_format(&self.format);
        let muxer = self.get_muxer_for_format(&self.format);

        let mut cmd = Command::new(&self.ffmpeg_path);
        cmd.args([
            "-i",
            input,
            "-f",
            muxer,
            "-acodec",
            codec,
            "-ab",
            &format!("{}k", self.bitrate),
            "-ar",
            &self.sample_rate.to_string(),
            "-ac",
            &self.channels.to_string(),
            "-loglevel",
            "error",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

        debug!("FFmpeg command: {:?}", cmd);

        let child = cmd.spawn()?;

        Ok(AudioProcess::new(child))
    }

    pub fn start_streaming_service(
        self,
        track_rx: Receiver<std::path::PathBuf>,
    ) -> Receiver<AudioChunk> {
        let (audio_tx, audio_rx) = unbounded::<AudioChunk>();

        tokio::spawn(async move {
            let mut current_process: Option<AudioProcess> = None;
            let mut current_track: Option<std::path::PathBuf> = None;
            let mut consecutive_failures: u32 = 0;

            loop {
                // Start new process if needed
                if current_process.is_none() {
                    // Try to get next track
                    if let Ok(track) = track_rx.try_recv() {
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
}
#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub data: Bytes,
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(calculate_backoff_ms(100), 30_000);
    }

    #[test]
    fn given_max_u32_failures_when_calculating_backoff_then_returns_max_backoff_ms() {
        assert_eq!(calculate_backoff_ms(u32::MAX), 30_000);
    }
}
