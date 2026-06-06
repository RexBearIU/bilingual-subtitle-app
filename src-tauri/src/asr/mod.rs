//! ASR subsystem.  `AudioChunk` is the boundary type between the VAD pipeline
//! and ASR; `whisper_server` is the concrete implementation.

/// A VAD-gated speech segment ready for transcription.
/// Samples are 16 kHz mono f32 PCM, including the pre-roll.
#[derive(Debug)]
pub struct AudioChunk {
    pub samples: Vec<f32>,
    /// Session-relative start time (ms), pre-roll included.
    pub started_at_ms: u64,
    /// Session-relative end time (ms).
    pub ended_at_ms: u64,
}

pub mod whisper_server;
