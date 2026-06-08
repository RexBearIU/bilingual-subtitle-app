//! Fixed-chunk audio batcher for ASR.
//!
//! ## Normal mode (video / stream)
//!
//! Two-phase emission per utterance:
//!
//! 1. **Partial flush** — after PARTIAL_FLUSH_SAMPLES (1 s) a partial chunk is
//!    sent immediately (`is_partial = true`).  ASR starts working while the speaker
//!    is still talking → partial subtitle appears ~1.5 s from speech start.
//!
//! 2. **Final flush** — triggered by silence (≥ SILENCE_FRAMES × 200 ms below
//!    SILENCE_RMS) or the 4 s cap.  Sends the remaining samples with the same
//!    utterance_id (`is_partial = false`) so the frontend replaces the partial
//!    in-place with the final transcription + translation.
//!
//! If there is no silence and the cap is reached, the whole 4 s goes as a single
//! non-partial chunk (no separate partial).
//!
//! ## Music mode
//! Fixed 10 s chunks; no partial or silence detection.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::Arc;
use std::time::Instant;

use crate::asr::AudioChunk;

const SAMPLE_RATE: usize = 16_000;

/// Maximum chunk for video / stream capture.
const CHUNK_SAMPLES: usize = 64_000; // 4 s

/// Music mode chunk size — longer window for full lyric lines.
const MUSIC_CHUNK_SAMPLES: usize = 160_000; // 10 s

/// Minimum samples for the stop-flush (avoid sending a near-empty WAV).
const MIN_FLUSH_SAMPLES: usize = SAMPLE_RATE / 2; // 0.5 s

/// Send a partial chunk after this many samples to start ASR early (video mode).
const PARTIAL_FLUSH_SAMPLES: usize = SAMPLE_RATE; // 1 s

/// RMS below this is considered silence (≈ −46 dBFS).
/// Conservative — only catches genuine quiet moments, not music dips.
const SILENCE_RMS: f32 = 0.005;

/// Consecutive ~200 ms blocks below SILENCE_RMS before a final flush.
const SILENCE_FRAMES: usize = 2; // ≈ 400 ms

pub fn start_vad_worker(
    rx: Receiver<Vec<f32>>,
    asr_tx: SyncSender<AudioChunk>,
    stop: Arc<AtomicBool>,
    _speech_threshold: f32, // kept for API compatibility — unused
    music_mode: Arc<AtomicBool>,
) {
    log::info!(
        "chunker: video={}s max / {}s partial / {}ms silence-flush  music={}s",
        CHUNK_SAMPLES / SAMPLE_RATE,
        PARTIAL_FLUSH_SAMPLES / SAMPLE_RATE,
        SILENCE_FRAMES * 200,
        MUSIC_CHUNK_SAMPLES / SAMPLE_RATE,
    );

    std::thread::Builder::new()
        .name("vad-worker".into())
        .spawn(move || chunk_loop(rx, asr_tx, &stop, &music_mode))
        .expect("spawn vad-worker thread");
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

fn send_chunk(
    asr_tx: &SyncSender<AudioChunk>,
    samples: Vec<f32>,
    utterance_id: u64,
    started_at_ms: u64,
    ended_at_ms: u64,
    is_partial: bool,
    seq: u64,
    tag: &str,
) {
    log::info!(
        "chunk [{}] u{} {}: {:.2}s  [{:.3}s–{:.3}s]{}",
        seq,
        utterance_id,
        if is_partial { "partial" } else { "final" },
        samples.len() as f64 / SAMPLE_RATE as f64,
        started_at_ms as f64 / 1000.0,
        ended_at_ms as f64 / 1000.0,
        if tag.is_empty() { String::new() } else { format!("  [{tag}]") },
    );
    match asr_tx.try_send(AudioChunk {
        samples,
        started_at_ms,
        ended_at_ms,
        utterance_id,
        is_partial,
    }) {
        Ok(_) => {}
        Err(std::sync::mpsc::TrySendError::Full(_)) => {
            log::warn!("ASR channel full — dropping chunk [{}]", seq);
        }
        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {}
    }
}

fn chunk_loop(
    rx: Receiver<Vec<f32>>,
    asr_tx: SyncSender<AudioChunk>,
    stop: &Arc<AtomicBool>,
    music_mode: &Arc<AtomicBool>,
) {
    let mut buf: Vec<f32> = Vec::new();
    let session_start = Instant::now();

    let mut utterance_id: u64 = 0;  // shared by partial + final of same utterance
    let mut utterance_started_ms: u64 = 0; // when the current utterance began
    let mut chunk_started_ms: u64 = 0;     // start of the current buf slice
    let mut seq: u64 = 0;                  // monotonic log counter

    // Video mode state
    let mut partial_sent: bool = false;
    let mut silence_count: usize = 0;
    let mut has_speech: bool = false;

    loop {
        if stop.load(Ordering::Relaxed) {
            if buf.len() >= MIN_FLUSH_SAMPLES {
                let ended_ms = session_start.elapsed().as_millis() as u64;
                seq += 1;
                if !partial_sent {
                    utterance_id += 1;
                }
                send_chunk(
                    &asr_tx,
                    std::mem::take(&mut buf),
                    utterance_id,
                    utterance_started_ms,
                    ended_ms,
                    false,
                    seq,
                    "stop-flush",
                );
            }
            break;
        }

        let audio = match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(a) => a,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };

        let now_ms = session_start.elapsed().as_millis() as u64;

        if buf.is_empty() {
            chunk_started_ms = now_ms;
            if !partial_sent {
                utterance_started_ms = now_ms;
            }
        }

        let is_music = music_mode.load(Ordering::Relaxed);

        if !is_music {
            if rms(&audio) >= SILENCE_RMS {
                silence_count = 0;
                has_speech = true;
            } else {
                silence_count += 1;
            }
        }

        buf.extend_from_slice(&audio);

        let target = if is_music { MUSIC_CHUNK_SAMPLES } else { CHUNK_SAMPLES };

        // ── Partial flush (video mode, 1 s, once per utterance) ──────────────
        if !is_music && !partial_sent && buf.len() >= PARTIAL_FLUSH_SAMPLES {
            utterance_id += 1;
            utterance_started_ms = chunk_started_ms;
            let partial: Vec<f32> = buf.drain(..PARTIAL_FLUSH_SAMPLES).collect();
            let partial_end_ms = now_ms;
            seq += 1;
            send_chunk(
                &asr_tx,
                partial,
                utterance_id,
                utterance_started_ms,
                partial_end_ms,
                true,
                seq,
                "",
            );
            partial_sent = true;
            chunk_started_ms = partial_end_ms;
        }

        // ── Final flush (silence or max) ──────────────────────────────────────
        let silence_flush = !is_music
            && has_speech
            && silence_count >= SILENCE_FRAMES
            && buf.len() >= MIN_FLUSH_SAMPLES;

        if buf.len() >= target || silence_flush {
            let drain = buf.len().min(target);
            let samples: Vec<f32> = buf.drain(..drain).collect();
            let ended_ms = session_start.elapsed().as_millis() as u64;
            seq += 1;

            if !partial_sent {
                utterance_id += 1;
                utterance_started_ms = chunk_started_ms;
            }

            let tag = if is_music {
                "music"
            } else if silence_flush {
                "silence"
            } else {
                "max"
            };

            send_chunk(
                &asr_tx,
                samples,
                utterance_id,
                utterance_started_ms,
                ended_ms,
                false,
                seq,
                tag,
            );

            chunk_started_ms = ended_ms;
            partial_sent = false;
            silence_count = 0;
            has_speech = false;
        }
    }

    log::info!("chunker exited");
}
