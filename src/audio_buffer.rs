use bytes::Bytes;
use crossbeam_channel::{bounded, Receiver, Sender};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

pub struct CircularBuffer {
    buffer: VecDeque<Bytes>,
    max_size: usize,
    total_bytes: usize,
    max_bytes: usize,
}

impl CircularBuffer {
    pub fn new(max_size: usize, max_bytes: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(max_size),
            max_size,
            total_bytes: 0,
            max_bytes,
        }
    }

    pub fn push(&mut self, data: Bytes) {
        while self.total_bytes + data.len() > self.max_bytes && !self.buffer.is_empty() {
            if let Some(removed) = self.buffer.pop_front() {
                self.total_bytes -= removed.len();
            }
        }

        if data.len() <= self.max_bytes {
            self.buffer.push_back(data.clone());
            self.total_bytes += data.len();

            while self.buffer.len() > self.max_size {
                if let Some(removed) = self.buffer.pop_front() {
                    self.total_bytes -= removed.len();
                }
            }
        }
    }

    pub fn pop(&mut self) -> Option<Bytes> {
        if let Some(data) = self.buffer.pop_front() {
            self.total_bytes -= data.len();
            Some(data)
        } else {
            None
        }
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

pub struct StreamBuffer {
    buffer: Arc<Mutex<CircularBuffer>>,
    input_sender: Sender<Bytes>,
    input_receiver: Receiver<Bytes>,
    running: Arc<Mutex<bool>>,
}

impl StreamBuffer {
    pub fn new(buffer_size: usize, max_buffer_bytes: usize) -> Self {
        let (input_sender, input_receiver) = bounded(1000);

        Self {
            buffer: Arc::new(Mutex::new(CircularBuffer::new(
                buffer_size,
                max_buffer_bytes,
            ))),
            input_sender,
            input_receiver,
            running: Arc::new(Mutex::new(false)),
        }
    }

    pub fn get_input_sender(&self) -> Sender<Bytes> {
        self.input_sender.clone()
    }

    pub fn start(&self) {
        let buffer = Arc::clone(&self.buffer);
        let receiver = self.input_receiver.clone();
        let running = Arc::clone(&self.running);

        {
            let mut running_guard = running.lock().unwrap();
            *running_guard = true;
        }

        tokio::spawn(async move {
            loop {
                let data = tokio::task::spawn_blocking({
                    let receiver = receiver.clone();
                    move || receiver.recv()
                })
                .await;

                match data {
                    Ok(Ok(bytes)) => {
                        let mut buffer_guard = buffer.lock().unwrap();
                        buffer_guard.push(bytes);
                    }
                    Ok(Err(_)) => break,
                    Err(_) => break,
                }
            }

            let mut running_guard = running.lock().unwrap();
            *running_guard = false;
        });
    }

    pub fn read_chunk(&self, max_size: usize) -> Option<Bytes> {
        let mut buffer_guard = self.buffer.lock().unwrap();

        if buffer_guard.is_empty() {
            return None;
        }

        let mut chunks = Vec::new();
        let mut total_size = 0;

        while let Some(chunk) = buffer_guard.pop() {
            let chunk_size = chunk.len();
            chunks.push(chunk);
            total_size += chunk_size;

            if total_size >= max_size {
                break;
            }
        }

        if chunks.is_empty() {
            None
        } else if chunks.len() == 1 {
            Some(chunks.into_iter().next().unwrap())
        } else {
            let mut combined = Vec::with_capacity(total_size);
            for chunk in chunks {
                combined.extend_from_slice(&chunk);
            }
            Some(Bytes::from(combined))
        }
    }

    pub fn buffer_info(&self) -> (usize, usize) {
        let buffer_guard = self.buffer.lock().unwrap();
        (buffer_guard.len(), buffer_guard.total_bytes())
    }

    pub fn is_running(&self) -> bool {
        let running_guard = self.running.lock().unwrap();
        *running_guard
    }
}

impl Clone for StreamBuffer {
    fn clone(&self) -> Self {
        Self {
            buffer: Arc::clone(&self.buffer),
            input_sender: self.input_sender.clone(),
            input_receiver: self.input_receiver.clone(),
            running: Arc::clone(&self.running),
        }
    }
}
