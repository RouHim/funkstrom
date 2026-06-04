use crate::audio_metadata::TrackMetadata;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackInfo {
    pub path: PathBuf,
    pub duration_secs: i64,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineSnapshot {
    pub track_path: PathBuf,
    pub track_index: usize,
    pub elapsed_in_track_secs: f64,
    pub generation: u64,
    pub current_metadata: TrackMetadata,
}

pub struct PlaybackDirector {
    pub playlist: Vec<TrackInfo>,
    pub current_index: usize,
    pub track_started_at: Instant,
    pub generation: u64,
    pub snapshot_tx: watch::Sender<TimelineSnapshot>,
    #[allow(dead_code)]
    pub listener_count: Arc<AtomicUsize>,
}

impl PlaybackDirector {
    pub fn new(playlist: Vec<TrackInfo>, listener_count: Arc<AtomicUsize>) -> Self {
        let track_started_at = Instant::now();
        let initial_snapshot = Self::snapshot_from_state(&playlist, 0, track_started_at, 0);
        let (snapshot_tx, _) = watch::channel(initial_snapshot);

        Self {
            playlist,
            current_index: 0,
            track_started_at,
            generation: 0,
            snapshot_tx,
            listener_count,
        }
    }

    pub fn tick(&mut self) {
        let mut changed = false;
        let now = Instant::now();

        if !self.playlist.is_empty() {
            changed |= self.skip_zero_duration_tracks(now);
            changed |= self.advance_finished_tracks(now);
        }

        let next_snapshot = Self::snapshot_from_state(
            &self.playlist,
            self.current_index,
            self.track_started_at,
            self.generation,
        );

        if changed || *self.snapshot_tx.borrow() != next_snapshot {
            self.snapshot_tx.send_replace(next_snapshot);
        }
    }

    #[allow(dead_code)]
    pub fn current_snapshot(&self) -> TimelineSnapshot {
        self.snapshot_tx.borrow().clone()
    }

    pub fn replace_playlist(&mut self, playlist: Vec<TrackInfo>) {
        self.playlist = playlist;
        self.current_index = 0;
        self.track_started_at = Instant::now();
        self.generation = self.generation.saturating_add(1);

        let next_snapshot = Self::snapshot_from_state(
            &self.playlist,
            self.current_index,
            self.track_started_at,
            self.generation,
        );
        self.snapshot_tx.send_replace(next_snapshot);
    }

    fn advance_finished_tracks(&mut self, now: Instant) -> bool {
        if self.playlist.is_empty() {
            return false;
        }

        let mut changed = false;

        loop {
            let duration_secs = self.playlist[self.current_index].duration_secs;

            if duration_secs <= 0 {
                break;
            }

            let elapsed = now.duration_since(self.track_started_at).as_secs_f64();
            if elapsed < duration_secs as f64 {
                break;
            }

            let overflow = elapsed - duration_secs as f64;
            self.current_index = (self.current_index + 1) % self.playlist.len();
            self.track_started_at = now - Duration::from_secs_f64(overflow.max(0.0));
            changed = true;

            if self.playlist[self.current_index].duration_secs <= 0 {
                if self.skip_zero_duration_tracks(now) {
                    changed = true;
                }
                if self.playlist[self.current_index].duration_secs <= 0 {
                    break;
                }
            }
        }

        changed
    }

    fn skip_zero_duration_tracks(&mut self, now: Instant) -> bool {
        if self.playlist.is_empty() {
            return false;
        }

        let mut changed = false;
        let mut checked = 0usize;

        while checked < self.playlist.len() && self.playlist[self.current_index].duration_secs <= 0
        {
            self.current_index = (self.current_index + 1) % self.playlist.len();
            self.track_started_at = now;
            checked += 1;
            changed = true;
        }

        changed
    }

    fn snapshot_from_state(
        playlist: &[TrackInfo],
        current_index: usize,
        track_started_at: Instant,
        generation: u64,
    ) -> TimelineSnapshot {
        if playlist.is_empty() {
            return TimelineSnapshot {
                track_path: PathBuf::new(),
                track_index: 0,
                elapsed_in_track_secs: 0.0,
                generation,
                current_metadata: TrackMetadata::default(),
            };
        }

        let index = current_index.min(playlist.len() - 1);
        let track_info = &playlist[index];

        let current_metadata = TrackMetadata {
            title: track_info
                .title
                .clone()
                .unwrap_or_else(|| "Unknown Track".to_string()),
            artist: track_info
                .artist
                .clone()
                .unwrap_or_else(|| "Unknown Artist".to_string()),
            album: track_info
                .album
                .clone()
                .unwrap_or_else(|| "Unknown Album".to_string()),
            file_path: track_info.path.to_string_lossy().to_string(),
        };

        TimelineSnapshot {
            track_path: playlist[index].path.clone(),
            track_index: index,
            elapsed_in_track_secs: Instant::now()
                .duration_since(track_started_at)
                .as_secs_f64(),
            generation,
            current_metadata,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::thread;
    use tokio::time::timeout;

    fn track(path: &str, duration_secs: i64) -> TrackInfo {
        TrackInfo {
            path: PathBuf::from(path),
            duration_secs,
            title: None,
            artist: None,
            album: None,
        }
    }

    #[test]
    fn given_elapsed_exceeds_track_duration_when_tick_then_advances_to_next_track() {
        let listeners = Arc::new(AtomicUsize::new(1));
        let mut director =
            PlaybackDirector::new(vec![track("a.mp3", 1), track("b.mp3", 10)], listeners);

        thread::sleep(Duration::from_millis(1100));
        director.tick();

        let snapshot = director.current_snapshot();
        assert_eq!(snapshot.track_path, PathBuf::from("b.mp3"));
        assert_eq!(snapshot.track_index, 1);
    }

    #[test]
    fn given_last_track_elapsed_when_tick_then_wraps_around_to_first_track() {
        let listeners = Arc::new(AtomicUsize::new(1));
        let mut director =
            PlaybackDirector::new(vec![track("a.mp3", 1), track("b.mp3", 1)], listeners);

        director.current_index = 1;
        director.track_started_at = Instant::now() - Duration::from_millis(1100);
        director.tick();

        let snapshot = director.current_snapshot();
        assert_eq!(snapshot.track_path, PathBuf::from("a.mp3"));
        assert_eq!(snapshot.track_index, 0);
    }

    #[test]
    fn given_empty_playlist_when_tick_then_does_not_panic_and_keeps_empty_snapshot() {
        let listeners = Arc::new(AtomicUsize::new(0));
        let mut director = PlaybackDirector::new(Vec::new(), listeners);

        director.tick();

        let snapshot = director.current_snapshot();
        assert_eq!(snapshot.track_path, PathBuf::new());
        assert_eq!(snapshot.track_index, 0);
        assert_eq!(snapshot.elapsed_in_track_secs, 0.0);
    }

    #[test]
    fn given_new_playlist_when_replaced_then_resets_index_and_increments_generation() {
        let listeners = Arc::new(AtomicUsize::new(1));
        let mut director = PlaybackDirector::new(vec![track("a.mp3", 10)], listeners);

        director.current_index = 0;
        director.track_started_at = Instant::now() - Duration::from_secs(5);
        director.replace_playlist(vec![track("x.mp3", 20), track("y.mp3", 20)]);

        let snapshot = director.current_snapshot();
        assert_eq!(snapshot.track_path, PathBuf::from("x.mp3"));
        assert_eq!(snapshot.track_index, 0);
        assert_eq!(snapshot.generation, 1);
        assert!(snapshot.elapsed_in_track_secs < 0.2);
    }

    #[test]
    fn given_active_track_when_tick_then_elapsed_matches_wall_clock_within_200ms() {
        let listeners = Arc::new(AtomicUsize::new(1));
        let mut director = PlaybackDirector::new(vec![track("a.mp3", 100)], listeners);

        thread::sleep(Duration::from_millis(300));
        director.tick();

        let snapshot = director.current_snapshot();
        assert!((snapshot.elapsed_in_track_secs - 0.3).abs() <= 0.2);
    }

    #[test]
    fn given_zero_duration_track_when_tick_then_skips_to_next_positive_duration_track() {
        let listeners = Arc::new(AtomicUsize::new(1));
        let mut director =
            PlaybackDirector::new(vec![track("zero.mp3", 0), track("ok.mp3", 10)], listeners);

        director.tick();

        let snapshot = director.current_snapshot();
        assert_eq!(snapshot.track_path, PathBuf::from("ok.mp3"));
        assert_eq!(snapshot.track_index, 1);
    }

    #[tokio::test]
    async fn given_no_listeners_when_tick_then_encoding_always_active() {
        let listeners = Arc::new(AtomicUsize::new(0));
        let mut director = PlaybackDirector::new(vec![track("a.mp3", 10)], listeners);
        let mut snapshot_rx = director.snapshot_tx.subscribe();

        director.tick();

        let _ = timeout(Duration::from_millis(200), snapshot_rx.changed())
            .await
            .expect("expected tick to publish snapshot")
            .expect("expected watch receiver to stay open");
        let snapshot = snapshot_rx.borrow().clone();
        assert_eq!(snapshot.track_path, PathBuf::from("a.mp3"));
        assert_eq!(snapshot.track_index, 0);
    }

    #[tokio::test]
    async fn given_listeners_disconnect_when_tick_then_encoding_stays_active() {
        let listeners = Arc::new(AtomicUsize::new(1));
        let mut director = PlaybackDirector::new(vec![track("a.mp3", 10)], listeners.clone());

        director.tick();
        assert_eq!(
            director.current_snapshot().track_path,
            PathBuf::from("a.mp3")
        );

        listeners.store(0, Ordering::SeqCst);
        director.tick();

        let snapshot = director.current_snapshot();
        assert_eq!(snapshot.track_path, PathBuf::from("a.mp3"));
        assert_eq!(snapshot.track_index, 0);
    }

    #[tokio::test]
    async fn given_zero_listeners_when_track_advances_then_snapshot_still_published() {
        let listeners = Arc::new(AtomicUsize::new(0));
        let mut director =
            PlaybackDirector::new(vec![track("a.mp3", 1), track("b.mp3", 10)], listeners);
        director.track_started_at = Instant::now() - Duration::from_millis(1100);
        let mut snapshot_rx = director.snapshot_tx.subscribe();

        director.tick();

        let _ = timeout(Duration::from_millis(200), snapshot_rx.changed())
            .await
            .expect("expected tick to publish advanced track snapshot")
            .expect("expected watch receiver to stay open");
        let snapshot = snapshot_rx.borrow().clone();

        assert_eq!(snapshot.track_path, PathBuf::from("b.mp3"));
        assert_eq!(snapshot.track_index, 1);
    }
}
