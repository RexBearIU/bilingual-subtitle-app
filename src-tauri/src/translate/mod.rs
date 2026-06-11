//! Translation pipeline: receives `TranslationRequest`s from the ASR worker
//! and calls llama-server (OpenAI-compatible API) to produce Traditional
//! Chinese subtitles.

pub mod llama_server;

use crate::types::SubtitleMode;

/// Sent from the ASR worker to the translation worker for each transcribed chunk.
pub struct TranslationRequest {
    /// Stable segment identifier — same as the source `subtitle_update` id.
    pub id: String,
    /// ISO-639-1 source language (`"ko"` / `"en"` / `"zh"`).
    pub source_lang: String,
    /// Source text as returned by asr-srv.
    pub source_text: String,
    /// Active subtitle display mode (drives which target language we need).
    pub mode: SubtitleMode,
    pub started_at_ms: u64,
    pub ended_at_ms: u64,
}
