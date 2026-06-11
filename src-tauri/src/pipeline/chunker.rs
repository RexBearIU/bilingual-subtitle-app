//! Fixed-chunk audio batcher for ASR.
//!
//! ## Normal mode (video / stream)
//!
//! Two-phase emission per utterance:
//!
//! 1. **Rolling partial flush** — after 1 s a COPY of the buffer is sent
//!    (`is_partial = true`), then an updated copy every further 1.5 s while
//!    the utterance continues.  The on-screen text keeps refreshing during
//!    long utterances.  The buffer is NOT drained so the final always has the
//!    full audio.
//!
//! 2. **Final flush** — triggered by a graduated silence rule (the more audio
//!    buffered, the shorter the pause needed: 800 ms under 1.5 s of audio,
//!    down to 200 ms past 2.5 s) or the 6 s hard cap (only reached when
//!    speech never pauses — fast talkers benefit from the longer Whisper
//!    context).  Sends the **complete utterance audio** with the same
//!    utterance_id (`is_partial = false`) so the frontend replaces the
//!    partial in-place with the final transcription + translation.
//!
//! ## Music mode
//! Fixed 10 s chunks; no partial or silence detection.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::Arc;
use std::time::Instant;

use crate::asr::AudioChunk;
use crate::audio::meter::rms;

const SAMPLE_RATE: usize = 16_000;

/// Hard cap for video / stream capture.  Only reached when speech never
/// pauses (fast talkers, no gaps) — more context per chunk is exactly what
/// Whisper needs there, and the rolling partials below keep the on-screen
/// latency low while the buffer grows.
const CHUNK_SAMPLES: usize = 96_000; // 6 s

/// Music mode chunk size — longer window for full lyric lines.
const MUSIC_CHUNK_SAMPLES: usize = 160_000; // 10 s

/// Minimum samples for the stop-flush (avoid sending a near-empty WAV).
const MIN_FLUSH_SAMPLES: usize = SAMPLE_RATE / 2; // 0.5 s

/// Send the first partial chunk after this many samples (video mode).
const PARTIAL_FLUSH_SAMPLES: usize = SAMPLE_RATE; // 1 s

/// Re-send an updated partial every additional 1.5 s while the utterance keeps
/// going, so long utterances show live text instead of a stale 1 s preview.
const PARTIAL_REFRESH_SAMPLES: usize = SAMPLE_RATE * 3 / 2; // 1.5 s

/// RMS below this is considered silence (≈ −46 dBFS).
/// Conservative — only catches genuine quiet moments, not music dips.
const SILENCE_RMS: f32 = 0.005;

/// Graduated silence flush: how many consecutive ~200 ms silent blocks are
/// required to end an utterance, given how much audio is already buffered.
///
/// Short buffer → demand a long pause, so a breath after 0.8 s doesn't produce
/// a useless fragment ("아.").  Long buffer → cut at the first real dip, so
/// utterances reach a natural boundary instead of the 4 s hard cap.
fn required_silence_frames(buf_len: usize) -> usize {
    if buf_len < SAMPLE_RATE * 3 / 2 {
        4 // < 1.5 s of audio: need ≈ 800 ms of silence
    } else if buf_len < SAMPLE_RATE * 5 / 2 {
        2 // 1.5 – 2.5 s: ≈ 400 ms
    } else {
        1 // ≥ 2.5 s: the first ≈ 200 ms dip is a good enough boundary
    }
}

pub fn start_vad_worker(
    rx: Receiver<Vec<f32>>,
    asr_tx: SyncSender<AudioChunk>,
    stop: Arc<AtomicBool>,
    _speech_threshold: f32, // kept for API compatibility — unused
    music_mode: Arc<AtomicBool>,
) {
    log::info!(
        "chunker: video={}s max / {}s partial / graduated silence-flush (800→200ms)  music={}s",
        CHUNK_SAMPLES / SAMPLE_RATE,
        PARTIAL_FLUSH_SAMPLES / SAMPLE_RATE,
        MUSIC_CHUNK_SAMPLES / SAMPLE_RATE,
    );

    std::thread::Builder::new()
        .name("vad-worker".into())
        .spawn(move || chunk_loop(rx, asr_tx, &stop, &music_mode))
        .expect("spawn vad-worker thread");
}

/// Pick a cut point for a max-cap flush: the centre of the quietest 50 ms
/// window within the last 1.5 s before `target`.  A hard cut at exactly
/// `target` regularly lands mid-syllable ("아름답" → "아름" + "답"), which both
/// garbles ASR output at the boundary and feeds the next chunk a word tail it
/// can't make sense of.  Cutting at the local energy minimum lands between
/// words/breaths most of the time.  The remainder stays in the buffer and
/// opens the next utterance.
fn quietest_cut(buf: &[f32], target: usize) -> usize {
    const WIN: usize = SAMPLE_RATE / 20;          // 50 ms
    const LOOKBACK: usize = SAMPLE_RATE * 3 / 2;  // search the last 1.5 s
    let hi = buf.len().min(target);
    let lo = hi.saturating_sub(LOOKBACK);
    if hi - lo < WIN * 2 {
        return hi;
    }
    let mut best_start = hi - WIN;
    let mut best_rms = f32::INFINITY;
    let mut start = lo;
    while start + WIN <= hi {
        let r = rms(&buf[start..start + WIN]);
        if r < best_rms {
            best_rms = r;
            best_start = start;
        }
        start += WIN;
    }
    best_start + WIN / 2
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
    let chunk = AudioChunk {
        samples,
        started_at_ms,
        ended_at_ms,
        utterance_id,
        is_partial,
    };
    if is_partial {
        // Partials are disposable previews — drop rather than block.
        match asr_tx.try_send(chunk) {
            Ok(_) => {}
            Err(std::sync::mpsc::TrySendError::Full(_)) => {
                log::debug!("ASR channel full — dropping partial [{}]", seq);
            }
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {}
        }
    } else {
        // Finals carry the actual subtitle — block briefly rather than lose
        // a whole utterance.  The ASR worker coalesces its backlog, so this
        // only stalls when inference is genuinely saturated.
        let _ = asr_tx.send(chunk);
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
    // Buffer length at which the next (rolling) partial fires.
    let mut next_partial: usize = PARTIAL_FLUSH_SAMPLES;
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

        // ── Rolling partial flush (video mode, 1 s then every 1.5 s) ─────────
        // Gated on has_speech: don't burn an ASR call previewing pure silence.
        // Clone, do NOT drain — the final must see the complete utterance audio
        // from the beginning so ASR doesn't start mid-sentence.
        if !is_music && has_speech && buf.len() >= next_partial {
            if !partial_sent {
                utterance_id += 1;
                utterance_started_ms = chunk_started_ms;
                partial_sent = true;
            }
            seq += 1;
            send_chunk(
                &asr_tx,
                buf.clone(),
                utterance_id,
                utterance_started_ms,
                now_ms,
                true,
                seq,
                "",
            );
            next_partial = buf.len() + PARTIAL_REFRESH_SAMPLES;
            // Don't update chunk_started_ms — the final starts from the same point.
        }

        // ── Final flush (silence or max) ──────────────────────────────────────
        let silence_flush = !is_music
            && has_speech
            && silence_count >= required_silence_frames(buf.len())
            && buf.len() >= MIN_FLUSH_SAMPLES;

        // Buffer hit the cap without any speech in it — pure silence/noise.
        // Discard instead of sending 4 s of nothing through ASR.
        if buf.len() >= target && !is_music && !has_speech {
            buf.clear();
            chunk_started_ms = now_ms;
            silence_count = 0;
            continue;
        }

        if buf.len() >= target || silence_flush {
            // Silence flush: the utterance ended naturally, take everything.
            // Max-cap flush: cut at the quietest spot near the cap so we don't
            // split a word in half; the remainder seeds the next utterance.
            let drain = if silence_flush {
                buf.len().min(target)
            } else {
                quietest_cut(&buf, target)
            };
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
            next_partial = PARTIAL_FLUSH_SAMPLES;
            silence_count = 0;
            has_speech = false;
        }
    }

    log::info!("chunker exited");
}
