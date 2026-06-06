use bytes::Bytes;
use std::collections::VecDeque;
use std::sync::{Arc, RwLock};

pub const DEFAULT_MAX_CHUNKS: usize = 1000;
pub const DEFAULT_MAX_BYTES: usize = 50 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct ListenerCursor {
    pub next_seq: u64,
}

#[derive(Debug)]
pub struct FanoutBuffer {
    pub ring: VecDeque<(u64, Bytes)>,
    pub next_seq: u64,
    pub oldest_seq: u64,
    pub max_chunks: usize,
    pub max_bytes: usize,
    pub current_bytes: usize,
}

pub type SharedFanoutBuffer = Arc<RwLock<FanoutBuffer>>;

impl Default for FanoutBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_CHUNKS, DEFAULT_MAX_BYTES)
    }
}

impl FanoutBuffer {
    pub fn new(max_chunks: usize, max_bytes: usize) -> Self {
        Self {
            ring: VecDeque::with_capacity(max_chunks),
            next_seq: 0,
            oldest_seq: 0,
            max_chunks,
            max_bytes,
            current_bytes: 0,
        }
    }

    pub fn push(&mut self, data: Bytes) {
        if self.max_chunks == 0 || self.max_bytes == 0 {
            self.ring.clear();
            self.current_bytes = 0;
            self.oldest_seq = self.next_seq;
            return;
        }

        if data.len() > self.max_bytes {
            log::warn!(
                "Dropped chunk larger than max_bytes: chunk={} max_bytes={}",
                data.len(),
                self.max_bytes
            );
            return;
        }

        let seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        self.current_bytes += data.len();
        self.ring.push_back((seq, data));

        while self.ring.len() > self.max_chunks || self.current_bytes > self.max_bytes {
            if let Some((evicted_seq, evicted)) = self.ring.pop_front() {
                self.current_bytes = self.current_bytes.saturating_sub(evicted.len());
                self.oldest_seq = evicted_seq.saturating_add(1);
            } else {
                break;
            }
        }

        if let Some((front_seq, _)) = self.ring.front() {
            self.oldest_seq = *front_seq;
        } else {
            self.oldest_seq = self.next_seq;
        }
    }

    pub fn read_from_cursor(&self, cursor: &mut ListenerCursor) -> Option<Bytes> {
        if cursor.next_seq < self.oldest_seq {
            log::warn!(
                "Listener cursor fell behind and was snapped forward: cursor_seq={} oldest_seq={}",
                cursor.next_seq,
                self.oldest_seq
            );
            cursor.next_seq = self.oldest_seq;
        }

        if cursor.next_seq >= self.next_seq {
            return None;
        }

        let offset = cursor.next_seq.saturating_sub(self.oldest_seq) as usize;
        let chunk = self
            .ring
            .get(offset)
            .map(|(seq, bytes)| {
                cursor.next_seq = seq.saturating_add(1);
                bytes.clone()
            })
            .or_else(|| {
                self.ring.iter().find_map(|(seq, bytes)| {
                    if *seq >= cursor.next_seq {
                        cursor.next_seq = seq.saturating_add(1);
                        Some(bytes.clone())
                    } else {
                        None
                    }
                })
            });

        if chunk.is_none() {
            cursor.next_seq = self.next_seq;
        }

        chunk
    }

    #[cfg(test)]
    pub fn new_cursor(&self) -> ListenerCursor {
        ListenerCursor {
            next_seq: self.next_seq,
        }
    }

    pub fn new_cursor_with_burst(&self, burst_chunks: usize) -> ListenerCursor {
        let burst_start = self.next_seq.saturating_sub(burst_chunks as u64);
        ListenerCursor {
            next_seq: burst_start.max(self.oldest_seq),
        }
    }

    pub fn shared(self) -> SharedFanoutBuffer {
        Arc::new(RwLock::new(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn bytes(s: &str) -> Bytes {
        Bytes::from(s.to_string())
    }

    #[test]
    fn given_empty_fanout_buffer_when_pushing_chunk_then_sequence_is_incremented() {
        let mut buffer = FanoutBuffer::new(10, 1024);

        buffer.push(bytes("a"));

        assert_eq!(buffer.next_seq, 1);
        assert_eq!(buffer.oldest_seq, 0);
        assert_eq!(buffer.ring.len(), 1);
    }

    #[test]
    fn given_two_cursors_when_reading_then_both_receive_same_data_non_destructively() {
        let mut buffer = FanoutBuffer::new(10, 1024);
        buffer.push(bytes("chunk-1"));
        buffer.push(bytes("chunk-2"));

        let mut cursor_a = ListenerCursor { next_seq: 0 };
        let mut cursor_b = ListenerCursor { next_seq: 0 };

        assert_eq!(
            buffer.read_from_cursor(&mut cursor_a),
            Some(bytes("chunk-1"))
        );
        assert_eq!(
            buffer.read_from_cursor(&mut cursor_b),
            Some(bytes("chunk-1"))
        );
        assert_eq!(
            buffer.read_from_cursor(&mut cursor_a),
            Some(bytes("chunk-2"))
        );
        assert_eq!(
            buffer.read_from_cursor(&mut cursor_b),
            Some(bytes("chunk-2"))
        );
    }

    #[test]
    fn given_full_fanout_buffer_when_pushing_more_then_oldest_chunk_is_evicted() {
        let mut buffer = FanoutBuffer::new(3, 1024);
        buffer.push(bytes("1"));
        buffer.push(bytes("2"));
        buffer.push(bytes("3"));
        buffer.push(bytes("4"));

        assert_eq!(buffer.ring.len(), 3);
        assert_eq!(buffer.oldest_seq, 1);

        let mut cursor = ListenerCursor { next_seq: 1 };
        assert_eq!(buffer.read_from_cursor(&mut cursor), Some(bytes("2")));
    }

    #[test]
    fn given_slow_cursor_behind_oldest_when_reading_then_cursor_snaps_forward() {
        let mut buffer = FanoutBuffer::new(2, 1024);
        buffer.push(bytes("1"));
        buffer.push(bytes("2"));
        buffer.push(bytes("3"));

        let mut cursor = ListenerCursor { next_seq: 0 };

        assert_eq!(buffer.read_from_cursor(&mut cursor), Some(bytes("2")));
        assert_eq!(cursor.next_seq, 2);
    }

    #[test]
    fn given_burst_cursor_when_created_then_it_starts_n_chunks_behind_write_head() {
        let mut buffer = FanoutBuffer::new(10, 1024);
        buffer.push(bytes("1"));
        buffer.push(bytes("2"));
        buffer.push(bytes("3"));
        buffer.push(bytes("4"));
        buffer.push(bytes("5"));

        let cursor = buffer.new_cursor_with_burst(2);

        assert_eq!(cursor.next_seq, 3);
    }

    #[test]
    fn given_cursor_at_write_head_when_reading_then_returns_none() {
        let mut buffer = FanoutBuffer::new(10, 1024);
        buffer.push(bytes("1"));

        let mut cursor = buffer.new_cursor();

        assert_eq!(buffer.read_from_cursor(&mut cursor), None);
    }

    #[test]
    fn given_shared_fanout_buffer_when_push_and_read_concurrently_then_no_corruption_occurs() {
        let shared = FanoutBuffer::new(200, 1024 * 1024).shared();

        let writer_buffer = Arc::clone(&shared);
        let writer = thread::spawn(move || {
            for i in 0..100u64 {
                let mut guard = writer_buffer.write().unwrap();
                guard.push(Bytes::from(format!("chunk-{i}")));
            }
        });

        let reader_buffer = Arc::clone(&shared);
        let reader = thread::spawn(move || {
            let mut cursor = ListenerCursor { next_seq: 0 };
            let mut received = Vec::new();

            while received.len() < 100 {
                let chunk = {
                    let guard = reader_buffer.read().unwrap();
                    guard.read_from_cursor(&mut cursor)
                };

                if let Some(data) = chunk {
                    received.push(data);
                } else {
                    thread::yield_now();
                }
            }

            received
        });

        writer.join().unwrap();
        let received = reader.join().unwrap();

        assert_eq!(received.len(), 100);
        assert_eq!(received.first(), Some(&bytes("chunk-0")));
        assert_eq!(received.last(), Some(&bytes("chunk-99")));
    }
}
