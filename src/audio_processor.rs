use bytes::Bytes;
use crossbeam_channel::{unbounded, Receiver};
use log::{debug, error, info, warn};
use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};

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

    pub fn check_ffmpeg_available(&self) -> Result<(), Box<dyn std::error::Error>> {
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
    ) -> Result<AudioProcess, Box<dyn std::error::Error>> {
        let input_str = input_path.to_str().unwrap();
        self.start_conversion(input_str)
    }

    pub fn start_conversion_from_url(
        &self,
        url: &str,
    ) -> Result<AudioProcess, Box<dyn std::error::Error>> {
        self.start_conversion(url)
    }

    fn start_conversion(&self, input: &str) -> Result<AudioProcess, Box<dyn std::error::Error>> {
        info!("Starting FFmpeg conversion for: {}", input);

        // Only check file existence for local files (not URLs)
        if !input.starts_with("http://") && !input.starts_with("https://") {
            let path = Path::new(input);
            if !path.exists() {
                return Err(format!("Input file does not exist: {}", input).into());
            }
        }

        let codec = self.get_codec_for_format(&self.format);

        let mut cmd = Command::new(&self.ffmpeg_path);
        cmd.args([
            "-i",
            input,
            "-f",
            &self.format,
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
                                info!("Started processing track: {:?}", track);
                                current_process = Some(process);
                            }
                            Err(e) => {
                                error!("Failed to start FFmpeg process for {:?}: {}", track, e);
                                continue;
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
                        }
                        Ok(None) => {
                            // Process finished
                            info!("Track processing completed: {:?}", current_track);
                            current_process = None;
                            current_track = None;
                        }
                        Err(e) => {
                            error!("Error reading from FFmpeg process: {}", e);
                            current_process = None;
                            current_track = None;
                        }
                    }
                }

                // Small delay to avoid busy waiting
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            }
        });

        audio_rx
    }
}

pub struct AudioProcess {
    child: Child,
    reader: Option<BufReader<std::process::ChildStdout>>,
}

impl AudioProcess {
    fn new(mut child: Child) -> Self {
        let reader = child.stdout.take().map(BufReader::new);
        Self { child, reader }
    }

    pub fn read_chunk(&mut self) -> Result<Option<Bytes>, Box<dyn std::error::Error>> {
        if let Some(ref mut reader) = self.reader {
            let mut buffer = [0u8; 8192]; // 8KB chunks

            match reader.read(&mut buffer) {
                Ok(0) => {
                    // EOF reached
                    self.wait_for_completion()?;
                    Ok(None)
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

    fn wait_for_completion(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        match self.child.wait() {
            Ok(status) => {
                if status.success() {
                    debug!("FFmpeg process completed successfully");
                } else {
                    warn!("FFmpeg process exited with status: {}", status);
                }
                Ok(())
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
}
