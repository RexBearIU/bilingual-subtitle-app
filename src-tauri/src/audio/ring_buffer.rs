//! Bounded ring buffer of f32 audio samples — used by the VAD pre-roll (M3).

/// Bounded ring buffer of f32 audio samples.
/// When capacity is exceeded the oldest samples are overwritten.
pub struct RingBuffer {
    buf: Box<[f32]>,
    /// Index of the next write position.
    write: usize,
    /// How many valid samples are currently held (≤ cap).
    filled: usize,
    cap: usize,
}

impl RingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: vec![0.0f32; capacity].into_boxed_slice(),
            write: 0,
            filled: 0,
            cap: capacity,
        }
    }

    pub fn push_slice(&mut self, samples: &[f32]) {
        for &s in samples {
            self.buf[self.write] = s;
            self.write = (self.write + 1) % self.cap;
            if self.filled < self.cap {
                self.filled += 1;
            }
        }
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.filled
    }

    #[allow(dead_code)]
    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// Copy the `n` most-recently-written samples (oldest-first) into `out`.
    pub fn read_last(&self, n: usize, out: &mut Vec<f32>) {
        let n = n.min(self.filled);
        out.clear();
        out.reserve(n);
        let start = (self.write + self.cap - n) % self.cap;
        for i in 0..n {
            out.push(self.buf[(start + i) % self.cap]);
        }
    }
}
