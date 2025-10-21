use crate::config::{ProgramType, ScheduleProgram};
use crate::m3u_parser::M3uParser;
use chrono::{DateTime, Duration, Local};
use cron::Schedule;
use crossbeam_channel::{unbounded, Receiver, Sender};
use log::{debug, error, info};
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub enum PlaylistCommand {
    SwitchToPlaylist {
        name: String,
        tracks: Vec<PathBuf>,
        duration: Duration,
    },
    SwitchToLiveset {
        name: String,
        genres: Vec<String>,
        duration: Duration,
    },
    ReturnToLibrary,
}

pub struct ScheduleEngine {
    programs: Vec<ValidatedProgram>,
    command_tx: Sender<PlaylistCommand>,
    command_rx: Receiver<PlaylistCommand>,
}

#[derive(Debug)]
struct ValidatedProgram {
    name: String,
    schedule: Schedule,
    duration: Duration,
    program_type: ProgramType,
    playlist_path: Option<PathBuf>,
    genres: Option<Vec<String>>,
}

impl ScheduleEngine {
    pub fn new(
        programs: Vec<ScheduleProgram>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let (command_tx, command_rx) = unbounded();

        let validated_programs = programs
            .into_iter()
            .filter(|p| p.active)
            .filter_map(|program| match Self::validate_and_convert(&program) {
                Ok(validated) => Some(validated),
                Err(e) => {
                    error!("Program '{}' skipped: {}", program.name, e);
                    None
                }
            })
            .collect::<Vec<_>>();

        if validated_programs.is_empty() {
            return Err("No active and valid programs found for scheduling".into());
        }

        info!(
            "Schedule engine initialized with {} active program(s)",
            validated_programs.len()
        );

        Ok(Self {
            programs: validated_programs,
            command_tx,
            command_rx,
        })
    }

    fn validate_and_convert(
        program: &ScheduleProgram,
    ) -> Result<ValidatedProgram, Box<dyn std::error::Error + Send + Sync>> {
        // Validate program-specific fields
        program
            .validate()
            .map_err(|e| format!("Program '{}': {}", program.name, e))?;

        let schedule = Schedule::from_str(&program.cron)
            .map_err(|e| format!("Invalid cron expression '{}': {}", program.cron, e))?;

        let duration = Self::parse_duration(&program.duration)?;

        let program_type = program.get_type();

        let playlist_path = match program_type {
            ProgramType::Playlist => {
                let path = PathBuf::from(
                    program
                        .playlist
                        .as_ref()
                        .expect("Playlist path should exist after validation"),
                );
                M3uParser::validate_playlist(&path)?;
                Some(path)
            }
            ProgramType::Liveset => None,
        };

        let genres = match program_type {
            ProgramType::Liveset => Some(
                program
                    .genres
                    .clone()
                    .expect("Genres should exist after validation"),
            ),
            ProgramType::Playlist => None,
        };

        Ok(ValidatedProgram {
            name: program.name.clone(),
            schedule,
            duration,
            program_type,
            playlist_path,
            genres,
        })
    }

    fn parse_duration(
        duration_str: &str,
    ) -> Result<Duration, Box<dyn std::error::Error + Send + Sync>> {
        let duration_str = duration_str.trim();

        if let Some(minutes_str) = duration_str.strip_suffix('m') {
            let minutes: i64 = minutes_str
                .parse()
                .map_err(|_| format!("Invalid duration format: {}", duration_str))?;
            return Ok(Duration::minutes(minutes));
        }

        if let Some(hours_str) = duration_str.strip_suffix('h') {
            let hours: i64 = hours_str
                .parse()
                .map_err(|_| format!("Invalid duration format: {}", duration_str))?;
            return Ok(Duration::hours(hours));
        }

        Err(format!(
            "Invalid duration format: {}. Use '30m' or '2h'",
            duration_str
        )
        .into())
    }

    pub fn get_command_receiver(&self) -> Receiver<PlaylistCommand> {
        self.command_rx.clone()
    }

    pub fn start(self) {
        tokio::spawn(async move {
            info!("Schedule engine started");
            let mut current_program: Option<(String, DateTime<Local>)> = None;

            loop {
                let now = Local::now();
                debug!("Schedule check at {}", now.format("%H:%M:%S"));

                // Calculate how long to sleep
                let sleep_duration = if let Some((ref program_name, end_time)) = current_program {
                    // A program is running, check if it should end
                    if now >= end_time {
                        info!("Program '{}' ended, returning to library", program_name);
                        if let Err(e) = self.command_tx.send(PlaylistCommand::ReturnToLibrary) {
                            error!("Failed to send return to library command: {}", e);
                        }
                        current_program = None;
                        std::time::Duration::from_secs(1) // Check again soon
                    } else {
                        // Sleep until the program ends (or check every 5 seconds, whichever is sooner)
                        let time_until_end = (end_time - now).num_seconds().max(0) as u64;
                        std::time::Duration::from_secs(time_until_end.min(5))
                    }
                } else {
                    // No program running, check for next scheduled program
                    if let Some((program, start_time)) = self.find_next_program(&now) {
                        // Allow a tolerance window: start if scheduled time is in the past but within last 2 seconds
                        let tolerance = Duration::seconds(2);
                        let earliest_start = now - tolerance;

                        if start_time >= earliest_start && start_time <= now {
                            // Start this program now
                            self.start_program(program, &now, &mut current_program);
                            std::time::Duration::from_secs(1) // Check again soon
                        } else {
                            // Calculate time until next program (or check every 30 seconds, whichever is sooner)
                            let time_until_start = (start_time - now).num_seconds().max(1) as u64; // Minimum 1 second
                            debug!(
                                "Next program '{}' starts in {} seconds",
                                program.name, time_until_start
                            );
                            std::time::Duration::from_secs(time_until_start.min(30))
                        }
                    } else {
                        // No programs scheduled, check again in 30 seconds
                        std::time::Duration::from_secs(30)
                    }
                };

                tokio::time::sleep(sleep_duration).await;
            }
        });
    }

    fn find_next_program(
        &self,
        now: &DateTime<Local>,
    ) -> Option<(&ValidatedProgram, DateTime<Local>)> {
        // Find the next scheduled program
        // Use `after()` instead of `upcoming()` to include times that are exactly now
        // `upcoming()` only returns strictly FUTURE times, so at 20:00:00 it returns 20:01:00
        // `after()` with a time slightly in the past includes the current minute

        let tolerance = Duration::seconds(2);
        let check_from = *now - tolerance;

        self.programs
            .iter()
            .filter_map(|program| {
                // Get the next occurrence after (now - tolerance)
                // This way, if we're at 20:00:01, we check from 19:59:59 and get 20:00:00
                let mut after_iter = program.schedule.after(&check_from);
                let next_time = after_iter.next()?;

                Some((program, next_time))
            })
            .min_by_key(|(_, next_time)| *next_time)
    }

    fn start_program(
        &self,
        program: &ValidatedProgram,
        now: &DateTime<Local>,
        current_program: &mut Option<(String, DateTime<Local>)>,
    ) {
        let end_time = *now + program.duration;

        match program.program_type {
            ProgramType::Playlist => {
                let playlist_path = program
                    .playlist_path
                    .as_ref()
                    .expect("Playlist path should exist for playlist programs");

                match M3uParser::parse(playlist_path) {
                    Ok(tracks) => {
                        info!(
                            "Starting playlist program '{}' with {} tracks (duration: {})",
                            program.name,
                            tracks.len(),
                            Self::format_duration(&program.duration)
                        );

                        if self
                            .command_tx
                            .send(PlaylistCommand::SwitchToPlaylist {
                                name: program.name.clone(),
                                tracks,
                                duration: program.duration,
                            })
                            .is_ok()
                        {
                            *current_program = Some((program.name.clone(), end_time));
                        } else {
                            error!("Failed to send playlist switch command");
                        }
                    }
                    Err(e) => {
                        error!(
                            "Failed to load playlist for program '{}': {}",
                            program.name, e
                        );
                    }
                }
            }
            ProgramType::Liveset => {
                let genres = program
                    .genres
                    .as_ref()
                    .expect("Genres should exist for liveset programs");

                info!(
                    "Starting liveset program '{}' (genres: {:?}, duration: {})",
                    program.name,
                    if genres.is_empty() {
                        "all".to_string()
                    } else {
                        genres.join(", ")
                    },
                    Self::format_duration(&program.duration)
                );

                if self
                    .command_tx
                    .send(PlaylistCommand::SwitchToLiveset {
                        name: program.name.clone(),
                        genres: genres.clone(),
                        duration: program.duration,
                    })
                    .is_ok()
                {
                    *current_program = Some((program.name.clone(), end_time));
                } else {
                    error!("Failed to send liveset switch command");
                }
            }
        }
    }

    fn format_duration(duration: &Duration) -> String {
        let hours = duration.num_hours();
        let minutes = duration.num_minutes() % 60;

        if hours > 0 {
            if minutes > 0 {
                format!("{}h {}m", hours, minutes)
            } else {
                format!("{}h", hours)
            }
        } else {
            format!("{}m", minutes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    #[test]
    fn given_duration_string_with_minutes_when_parsed_then_returns_correct_duration() {
        let result = ScheduleEngine::parse_duration("30m").unwrap();

        assert_eq!(result, Duration::minutes(30));
    }

    #[test]
    fn given_duration_string_with_hours_when_parsed_then_returns_correct_duration() {
        let result = ScheduleEngine::parse_duration("2h").unwrap();

        assert_eq!(result, Duration::hours(2));
    }

    #[test]
    fn given_duration_string_with_whitespace_when_parsed_then_trims_and_parses_correctly() {
        let result = ScheduleEngine::parse_duration(" 45m ").unwrap();

        assert_eq!(result, Duration::minutes(45));
    }

    #[test]
    fn given_duration_without_suffix_when_parsed_then_returns_error() {
        let result = ScheduleEngine::parse_duration("30");

        assert!(result.is_err());
    }

    #[test]
    fn given_duration_with_invalid_suffix_when_parsed_then_returns_error() {
        let result = ScheduleEngine::parse_duration("30s");

        assert!(result.is_err());
    }

    #[test]
    fn given_duration_with_non_numeric_value_when_parsed_then_returns_error() {
        let result = ScheduleEngine::parse_duration("abcm");

        assert!(result.is_err());
    }

    #[test]
    fn given_duration_in_minutes_when_formatted_then_returns_minutes_string() {
        let duration = Duration::minutes(45);

        let formatted = ScheduleEngine::format_duration(&duration);

        assert_eq!(formatted, "45m");
    }

    #[test]
    fn given_duration_in_exact_hours_when_formatted_then_returns_hours_string() {
        let duration = Duration::hours(2);

        let formatted = ScheduleEngine::format_duration(&duration);

        assert_eq!(formatted, "2h");
    }

    #[test]
    fn given_duration_in_hours_and_minutes_when_formatted_then_returns_combined_string() {
        let duration = Duration::minutes(150);

        let formatted = ScheduleEngine::format_duration(&duration);

        assert_eq!(formatted, "2h 30m");
    }

    #[test]
    fn given_program_with_invalid_cron_when_validated_then_returns_error_about_cron() {
        let program = ScheduleProgram {
            name: "test".to_string(),
            active: true,
            cron: "invalid cron".to_string(),
            duration: "30m".to_string(),
            program_type: Some("playlist".to_string()),
            playlist: Some("test.m3u".to_string()),
            genres: None,
        };

        let result = ScheduleEngine::validate_and_convert(&program);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid cron"));
    }

    #[test]
    fn given_program_with_invalid_duration_when_validated_then_returns_error_about_duration() {
        let program = ScheduleProgram {
            name: "test".to_string(),
            active: true,
            cron: "0 0 * * * *".to_string(),
            duration: "invalid".to_string(),
            program_type: Some("playlist".to_string()),
            playlist: Some("test.m3u".to_string()),
            genres: None,
        };

        let result = ScheduleEngine::validate_and_convert(&program);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid duration format"));
    }

    #[test]
    fn given_program_scheduled_at_exact_minute_when_queried_at_same_time_then_finds_program() {
        // Test that a program scheduled at exactly 20:00:00 is found when queried at 20:00:00
        let program = ScheduleProgram {
            name: "exact_time".to_string(),
            active: true,
            cron: "0 0 20 * * *".to_string(), // Every day at 20:00:00
            duration: "1h".to_string(),
            program_type: Some("playlist".to_string()),
            playlist: Some("test.m3u".to_string()),
            genres: None,
        };

        // Create a minimal test file for validation
        use tempfile::NamedTempFile;
        let temp_track = NamedTempFile::new().unwrap();
        let temp_file = NamedTempFile::new().unwrap();
        std::fs::write(
            temp_file.path(),
            format!("{}\n", temp_track.path().to_string_lossy()),
        )
        .unwrap();

        let mut program = program;
        program.playlist = Some(temp_file.path().to_string_lossy().to_string());

        let engine = ScheduleEngine::new(vec![program]).unwrap();

        // Query at exactly 20:00:00
        let now = Local::now()
            .date_naive()
            .and_hms_opt(20, 0, 0)
            .unwrap()
            .and_local_timezone(Local)
            .unwrap();

        let result = engine.find_next_program(&now);

        // Should find the program within tolerance window
        assert!(result.is_some());
        let (found_program, scheduled_time) = result.unwrap();
        assert_eq!(found_program.name, "exact_time");

        // Scheduled time should be at 20:00:00
        assert_eq!(scheduled_time.hour(), 20);
        assert_eq!(scheduled_time.minute(), 0);
        assert_eq!(scheduled_time.second(), 0);
        // Files automatically cleaned up when temp_track and temp_file drop
    }

    #[test]
    fn given_program_scheduled_when_queried_within_tolerance_then_finds_program() {
        // Test that a program scheduled at 20:00:00 is found when queried at 20:00:01
        let program = ScheduleProgram {
            name: "tolerance_test".to_string(),
            active: true,
            cron: "0 0 20 * * *".to_string(),
            duration: "30m".to_string(),
            program_type: Some("playlist".to_string()),
            playlist: Some("test.m3u".to_string()),
            genres: None,
        };

        use tempfile::NamedTempFile;
        let temp_track = NamedTempFile::new().unwrap();
        let temp_file = NamedTempFile::new().unwrap();
        std::fs::write(
            temp_file.path(),
            format!("{}\n", temp_track.path().to_string_lossy()),
        )
        .unwrap();

        let mut program = program;
        program.playlist = Some(temp_file.path().to_string_lossy().to_string());

        let engine = ScheduleEngine::new(vec![program]).unwrap();

        // Query at 20:00:01 (1 second after scheduled time)
        let now = Local::now()
            .date_naive()
            .and_hms_opt(20, 0, 1)
            .unwrap()
            .and_local_timezone(Local)
            .unwrap();

        let result = engine.find_next_program(&now);

        // Should still find the program within 2-second tolerance
        assert!(result.is_some());
        let (found_program, scheduled_time) = result.unwrap();
        assert_eq!(found_program.name, "tolerance_test");

        // Scheduled time should be at 20:00:00 (the original time)
        assert_eq!(scheduled_time.hour(), 20);
        assert_eq!(scheduled_time.minute(), 0);
        assert_eq!(scheduled_time.second(), 0);
        // Files automatically cleaned up when temp_track and temp_file drop
    }

    #[test]
    fn given_program_scheduled_when_queried_outside_tolerance_then_finds_next_occurrence() {
        // Test that a program scheduled at 20:00:00 is NOT found when queried at 20:00:03
        let program = ScheduleProgram {
            name: "outside_tolerance".to_string(),
            active: true,
            cron: "0 0 20 * * *".to_string(),
            duration: "30m".to_string(),
            program_type: Some("playlist".to_string()),
            playlist: Some("test.m3u".to_string()),
            genres: None,
        };

        use tempfile::NamedTempFile;
        let temp_track = NamedTempFile::new().unwrap();
        let temp_file = NamedTempFile::new().unwrap();
        std::fs::write(
            temp_file.path(),
            format!("{}\n", temp_track.path().to_string_lossy()),
        )
        .unwrap();

        let mut program = program;
        program.playlist = Some(temp_file.path().to_string_lossy().to_string());

        let engine = ScheduleEngine::new(vec![program]).unwrap();

        // Query at 20:00:03 (3 seconds after scheduled time, outside 2-second tolerance)
        let now = Local::now()
            .date_naive()
            .and_hms_opt(20, 0, 3)
            .unwrap()
            .and_local_timezone(Local)
            .unwrap();

        let result = engine.find_next_program(&now);

        if let Some((_, scheduled_time)) = result {
            // If found, it should be the next occurrence (tomorrow at 20:00:00)
            assert!(scheduled_time > now);
            // Should be more than 23 hours away
            assert!((scheduled_time - now).num_hours() >= 23);
        }
        // Files automatically cleaned up when temp_track and temp_file drop
    }

    #[test]
    fn given_multiple_programs_when_finding_next_then_returns_nearest_program() {
        // Test that when multiple programs are scheduled, the nearest one is returned
        let program1 = ScheduleProgram {
            name: "program1".to_string(),
            active: true,
            cron: "0 0 21 * * *".to_string(), // 21:00:00
            duration: "1h".to_string(),
            program_type: Some("playlist".to_string()),
            playlist: Some("test1.m3u".to_string()),
            genres: None,
        };

        let program2 = ScheduleProgram {
            name: "program2".to_string(),
            active: true,
            cron: "0 30 20 * * *".to_string(), // 20:30:00
            duration: "30m".to_string(),
            program_type: Some("playlist".to_string()),
            playlist: Some("test2.m3u".to_string()),
            genres: None,
        };

        use tempfile::NamedTempFile;
        let temp_track1 = NamedTempFile::new().unwrap();
        let temp_track2 = NamedTempFile::new().unwrap();

        let temp_file1 = NamedTempFile::new().unwrap();
        let temp_file2 = NamedTempFile::new().unwrap();
        std::fs::write(
            temp_file1.path(),
            format!("{}\n", temp_track1.path().to_string_lossy()),
        )
        .unwrap();
        std::fs::write(
            temp_file2.path(),
            format!("{}\n", temp_track2.path().to_string_lossy()),
        )
        .unwrap();

        let mut program1 = program1;
        let mut program2 = program2;
        program1.playlist = Some(temp_file1.path().to_string_lossy().to_string());
        program2.playlist = Some(temp_file2.path().to_string_lossy().to_string());

        let engine = ScheduleEngine::new(vec![program1, program2]).unwrap();

        // Query at 20:00:00
        let now = Local::now()
            .date_naive()
            .and_hms_opt(20, 0, 0)
            .unwrap()
            .and_local_timezone(Local)
            .unwrap();

        let result = engine.find_next_program(&now);

        // Should find program2 (20:30:00) as it's the nearest future program
        assert!(result.is_some());
        let (found_program, scheduled_time) = result.unwrap();
        assert_eq!(found_program.name, "program2");
        assert_eq!(scheduled_time.hour(), 20);
        assert_eq!(scheduled_time.minute(), 30);
        // Files automatically cleaned up when temp files drop
    }

    #[test]
    fn given_future_program_when_finding_next_then_returns_next_occurrence() {
        // Test behavior when no programs are scheduled for today
        // This tests the case where find_next_program returns None
        let program = ScheduleProgram {
            name: "future_program".to_string(),
            active: true,
            // Scheduled for a very specific time that's unlikely to match
            cron: "0 37 3 1 1 *".to_string(), // Jan 1st at 03:37:00
            duration: "1h".to_string(),
            program_type: Some("playlist".to_string()),
            playlist: Some("test.m3u".to_string()),
            genres: None,
        };

        use tempfile::NamedTempFile;
        let temp_track = NamedTempFile::new().unwrap();
        let temp_file = NamedTempFile::new().unwrap();
        std::fs::write(
            temp_file.path(),
            format!("{}\n", temp_track.path().to_string_lossy()),
        )
        .unwrap();

        let mut program = program;
        program.playlist = Some(temp_file.path().to_string_lossy().to_string());

        let engine = ScheduleEngine::new(vec![program]).unwrap();

        // Query at a time that doesn't match
        let now = Local::now();

        let result = engine.find_next_program(&now);

        // Should find the next occurrence in the future
        assert!(result.is_some());
        let (_, scheduled_time) = result.unwrap();

        // Scheduled time should be in the future
        assert!(scheduled_time > now);
        // Files automatically cleaned up when temp_track and temp_file drop
    }
}
