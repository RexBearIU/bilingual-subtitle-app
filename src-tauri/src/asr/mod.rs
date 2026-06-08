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
    /// Shared by all partial + final chunks from the same utterance.
    /// The ASR worker uses this as the subtitle slot ID so that partial
    /// transcriptions update in-place rather than stacking up.
    pub utterance_id: u64,
    /// `true` = mid-speech 5 s flush (transcribe + display, but skip translation).
    /// `false` = silence-terminated final chunk (transcribe + display + translate).
    pub is_partial: bool,
}

pub mod whisper_server;
