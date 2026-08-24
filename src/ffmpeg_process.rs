use bytes::Bytes;
use log::{debug, error, warn};
use std::io::{BufReader, Read};
use std::path::Path;
use std::process::Child;
// Constants for audio processing configuration
const AUDIO_CHUNK_SIZE: usize = 8192; // 8KB chunks for reading audio data
const INITIAL_BACKOFF_MS: u64 = 1000;
const MAX_BACKOFF_MS: u64 = 5_000;

/// Heuristic only: returns true for any value containing '/'. This means URLs
/// (e.g. "https://host/x") also match; callers must not treat this as a strict
/// filesystem-path check. resolve_ffmpeg_path's fallback logic depends on this
/// exact truth table — do not tighten the predicate without updating it.
pub(crate) fn looks_like_filesystem_path(value: &str) -> bool {
    value.starts_with('/') || value.contains('/')
}

pub(crate) fn resolve_ffmpeg_path(
    configured_path: Option<String>,
    env_ffmpeg_path: Option<String>,
) -> String {
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

pub(crate) fn calculate_backoff_ms(consecutive_failures: u32) -> u64 {
    let backoff =
        INITIAL_BACKOFF_MS.saturating_mul(1u64.wrapping_shl(consecutive_failures.min(15)));
    backoff.min(MAX_BACKOFF_MS)
}

pub(crate) fn is_url_input(input: &str) -> bool {
    input.starts_with("http://") || input.starts_with("https://")
}

pub(crate) struct AudioProcess {
    child: Child,
    reader: Option<BufReader<std::process::ChildStdout>>,
    stderr: Option<BufReader<std::process::ChildStderr>>,
}

impl AudioProcess {
    pub(crate) fn new(mut child: Child) -> Self {
        let reader = child.stdout.take().map(BufReader::new);
        let stderr = child.stderr.take().map(BufReader::new);
        Self {
            child,
            reader,
            stderr,
        }
    }

    pub(crate) fn read_chunk(
        &mut self,
    ) -> Result<Option<Bytes>, Box<dyn std::error::Error + Send + Sync>> {
        if let Some(ref mut reader) = self.reader {
            let mut buffer = [0u8; AUDIO_CHUNK_SIZE];
            match reader.read(&mut buffer) {
                Ok(0) => {
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

    pub(crate) fn terminate(mut self) {
        if let Err(e) = self.child.kill() {
            debug!("Unable to kill FFmpeg process cleanly: {}", e);
        }
        if let Err(e) = self.child.wait() {
            debug!("Unable to wait for killed FFmpeg process: {}", e);
        }
    }
}
#[derive(Debug, Clone)]
pub(crate) struct AudioChunk {
    pub(crate) data: Bytes,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_processor::tests::create_fake_ffmpeg_script;
    use crate::audio_processor::FFmpegProcessor;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::time::Duration;

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
