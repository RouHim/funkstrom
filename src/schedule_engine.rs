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
    pub fn new(programs: Vec<ScheduleProgram>) -> Result<Self, Box<dyn std::error::Error>> {
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
    ) -> Result<ValidatedProgram, Box<dyn std::error::Error>> {
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

    fn parse_duration(duration_str: &str) -> Result<Duration, Box<dyn std::error::Error>> {
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

                if let Some((ref program_name, end_time)) = current_program {
                    if now >= end_time {
                        info!("Program '{}' ended, returning to library", program_name);
                        if let Err(e) = self.command_tx.send(PlaylistCommand::ReturnToLibrary) {
                            error!("Failed to send return to library command: {}", e);
                        }
                        current_program = None;
                    }
                } else if let Some((program, start_time)) = self.find_next_program() {
                    if start_time <= now {
                        self.start_program(program, &now, &mut current_program);
                    }
                }

                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        });
    }

    fn find_next_program(&self) -> Option<(&ValidatedProgram, DateTime<Local>)> {
        self.programs
            .iter()
            .filter_map(|program| {
                program
                    .schedule
                    .upcoming(Local)
                    .next()
                    .map(|next_time| (program, next_time))
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
}
