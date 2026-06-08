//! RMS VAD — gates audio chunks for ASR.
//!
//! Receives 16 kHz mono f32 samples from the capture thread, processes them
//! in 25 ms frames, and forwards completed speech segments as `AudioChunk`s
//! to the ASR worker.
//!
//! ## Adaptive threshold
//! When `speech_threshold == 0`, the VAD adapts to the ambient noise level
//! automatically.  It maintains an exponential moving average (EMA) of the RMS
//! of quiet frames and sets the effective gate at `noise_ema × 4`, clamped to
//! the range [0.003, 0.12].  The EMA is only updated from frames that are not
//! already classified as speech, so continuous loud music does not inflate the
//! estimate.
//!
//! ## Partial flush
//! If speech has been ongoing for more than `PARTIAL_FLUSH_SECS` (5 s) without
//! a silence gap, the accumulated audio is flushed to the ASR worker immediately
//! as a partial chunk (`is_partial = true`).  This prevents long sentences from
//! waiting for silence before any subtitle appears.  The last 300 ms of audio is
//! kept as context for the next chunk so Whisper has continuity.
//!
//! ## Music mode
//! In music mode the VAD is bypassed entirely: audio is collected into fixed
//! 10-second chunks so continuous songs are not cut on silence gaps.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::Arc;
use std::time::Instant;

use crate::asr::AudioChunk;
use crate::audio::meter::rms;
use crate::audio::ring_buffer::RingBuffer;

/// 16 kHz frames of 25 ms each.
const SAMPLE_RATE: usize = 16_000;
const FRAME_SAMPLES: usize = 400; // 25 ms

/// Pre-roll included at the start of every chunk (300 ms).
const PREROLL_SAMPLES: usize = 4_800;

/// Number of consecutive silent frames before the current utterance is ended.
/// 16 × 25 ms = 400 ms of silence required.
const SILENCE_FRAMES: usize = 16;

/// Number of consecutive frames above threshold required before speech is
/// declared started.  This prevents music beats, game sound effects, and other
/// brief transients from opening a new utterance.  3 × 25 ms = 75 ms minimum.
const SPEECH_ONSET_FRAMES: usize = 3;

/// Hard cap on a single VAD chunk.  Should not normally be hit because the
/// partial-flush mechanism kicks in well before this.
const MAX_CHUNK_SAMPLES: usize = 192_000; // 12 s

/// Flush a partial chunk to ASR after this many samples of continuous speech.
/// User sees source-text subtitles every 5 s without waiting for silence.
const PARTIAL_FLUSH_SAMPLES: usize = 80_000; // 5 s

/// Overlap kept at the end of a partial flush so the next chunk has acoustic
/// context for Whisper's encoder (avoids cut-word artefacts).
const PARTIAL_OVERLAP_SAMPLES: usize = 4_800; // 300 ms

/// Music mode: send a chunk every 10 seconds regardless of silence.
const MUSIC_CHUNK_SAMPLES: usize = 160_000; // 10 s

/// Adaptive noise-floor EMA smoothing factor.
/// α ≈ 0.002 means ~500 quiet frames (~12 s) to reach 63 % of true value.
const NOISE_EMA_ALPHA: f32 = 0.002;

/// Absolute floor for the adaptive threshold (−50 dBFS).
const ADAPTIVE_THRESHOLD_MIN: f32 = 0.003;
/// Absolute ceiling for the adaptive threshold (−18 dBFS).
const ADAPTIVE_THRESHOLD_MAX: f32 = 0.12;
/// Multiplier applied to the noise EMA to get the gate threshold.
const NOISE_MULTIPLIER: f32 = 4.0;

struct SpeechAccum {
    samples: Vec<f32>,
    started_at_ms: u64,
    silent_frames: usize,
    /// Identifies which utterance this belongs to (shared by partial + final).
    utterance_id: u64,
}

/// Spawn the VAD worker thread (detached).
///
/// `speech_threshold`:
///   - `0.0` → fully automatic (adaptive noise-floor EMA, recommended)
///   - `> 0.0` → fixed RMS threshold (manual override, e.g. 0.032 = −30 dBFS)
///
/// `music_mode` is an `Arc<AtomicBool>` shared with `AppState` so the user
/// can toggle music mode live without restarting the pipeline.
pub fn start_vad_worker(
    rx: Receiver<Vec<f32>>,
    asr_tx: SyncSender<AudioChunk>,
    stop: Arc<AtomicBool>,
    speech_threshold: f32,
    music_mode: Arc<AtomicBool>,
) {
    let mode = if speech_threshold > 0.001 {
        format!("fixed {speech_threshold:.4} ({:.1} dBFS)", 20.0_f32 * speech_threshold.log10())
    } else {
        "adaptive (auto)".to_string()
    };
    log::info!("VAD: threshold={mode}");

    std::thread::Builder::new()
        .name("vad-worker".into())
        .spawn(move || vad_loop(rx, asr_tx, &stop, speech_threshold, &music_mode))
        .expect("spawn vad-worker thread");
}

fn vad_loop(
    rx: Receiver<Vec<f32>>,
    asr_tx: SyncSender<AudioChunk>,
    stop: &Arc<AtomicBool>,
    base_threshold: f32,
    music_mode: &Arc<AtomicBool>,
) {
    let mut preroll = RingBuffer::new(PREROLL_SAMPLES);
    let mut pending: Vec<f32> = Vec::new();
    let mut speech: Option<SpeechAccum> = None;
    let session_start = Instant::now();

    // Per-utterance counter: increments every time new speech starts.
    let mut utterance_counter: u64 = 0;
    // Consecutive above-threshold frames during pre-speech (onset detection).
    let mut onset_count: usize = 0;

    // Adaptive noise floor — exponential moving average of quiet-frame RMS.
    // Initialised to ~−34 dBFS so the gate starts at a sensible level.
    let mut noise_ema: f32 = 0.02;
    // Log the adaptive threshold periodically to aid debugging.
    let mut last_thr_log = Instant::now();

    // Music-mode accumulator.
    let mut music_buf: Vec<f32> = Vec::new();
    let mut music_started_ms: u64 = 0;

    loop {
        if stop.load(Ordering::Relaxed) {
            // Flush any in-progress speech chunk before exit.
            if let Some(accum) = speech.take() {
                if accum.samples.len() > FRAME_SAMPLES {
                    let ended_at_ms = session_start.elapsed().as_millis() as u64;
                    let _ = asr_tx.try_send(AudioChunk {
                        samples: accum.samples,
                        started_at_ms: accum.started_at_ms,
                        ended_at_ms,
                        utterance_id: accum.utterance_id,
                        is_partial: false,
                    });
                }
            }
            break;
        }

        let chunk = match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(c) => c,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };

        if music_mode.load(Ordering::Relaxed) {
            // ── Music mode: fixed 10 s chunks, no VAD ─────────────────────
            let now_ms = session_start.elapsed().as_millis() as u64;
            if music_buf.is_empty() { music_started_ms = now_ms; }
            music_buf.extend_from_slice(&chunk);

            while music_buf.len() >= MUSIC_CHUNK_SAMPLES {
                let samples: Vec<f32> = music_buf.drain(..MUSIC_CHUNK_SAMPLES).collect();
                let ended_at_ms = session_start.elapsed().as_millis() as u64;
                log::info!(
                    "Music chunk: {} samples  [{:.3}s – {:.3}s]",
                    samples.len(),
                    music_started_ms as f64 / 1000.0,
                    ended_at_ms as f64 / 1000.0,
                );
                utterance_counter += 1;
                match asr_tx.try_send(AudioChunk {
                    samples,
                    started_at_ms: music_started_ms,
                    ended_at_ms,
                    utterance_id: utterance_counter,
                    is_partial: false,
                }) {
                    Ok(_) => {}
                    Err(std::sync::mpsc::TrySendError::Full(_)) => {
                        log::warn!("Music mode: ASR channel full, chunk dropped");
                    }
                    Err(std::sync::mpsc::TrySendError::Disconnected(_)) => break,
                }
                music_started_ms = ended_at_ms;
            }
            speech = None;
        } else {
            // ── Speech mode ────────────────────────────────────────────────
            music_buf.clear();

            // Compute the effective threshold for this batch of frames.
            let effective_threshold = if base_threshold > 0.001 {
                base_threshold
            } else {
                (noise_ema * NOISE_MULTIPLIER).clamp(ADAPTIVE_THRESHOLD_MIN, ADAPTIVE_THRESHOLD_MAX)
            };

            // Log adaptive threshold changes every 10 s.
            if base_threshold <= 0.001 && last_thr_log.elapsed().as_secs() >= 10 {
                log::debug!(
                    "VAD adaptive: noise_ema={noise_ema:.5} threshold={effective_threshold:.4} ({:.1} dBFS)",
                    20.0_f32 * effective_threshold.log10(),
                );
                last_thr_log = Instant::now();
            }

            pending.extend_from_slice(&chunk);
            while pending.len() >= FRAME_SAMPLES {
                let frame: Vec<f32> = pending.drain(..FRAME_SAMPLES).collect();
                let frame_rms = rms(&frame);

                // Update adaptive noise EMA only from frames outside an active
                // speech segment (so foreground audio doesn't raise the floor).
                if base_threshold <= 0.001 && speech.is_none() {
                    noise_ema = NOISE_EMA_ALPHA * frame_rms + (1.0 - NOISE_EMA_ALPHA) * noise_ema;
                }

                if let Some(out) = process_frame(
                    &frame,
                    frame_rms,
                    &mut speech,
                    &mut preroll,
                    &session_start,
                    effective_threshold,
                    &mut utterance_counter,
                    &mut onset_count,
                ) {
                    match asr_tx.try_send(out) {
                        Ok(_) => {}
                        Err(std::sync::mpsc::TrySendError::Full(_)) => {
                            log::warn!("VAD: ASR channel full, dropping chunk");
                        }
                        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => break,
                    }
                }
            }
        }
    }

    log::info!("VAD worker exited");
}

fn process_frame(
    frame: &[f32],
    frame_rms: f32,           // pre-computed (also used for noise EMA in caller)
    speech: &mut Option<SpeechAccum>,
    preroll: &mut RingBuffer,
    session_start: &Instant,
    threshold: f32,
    utterance_counter: &mut u64,
    onset_count: &mut usize,  // consecutive frames above threshold (for onset detection)
) -> Option<AudioChunk> {
    let is_speech = frame_rms >= threshold;
    let now_ms = session_start.elapsed().as_millis() as u64;

    if let Some(ref mut accum) = speech {
        accum.samples.extend_from_slice(frame);
        if is_speech {
            accum.silent_frames = 0;
        } else {
            accum.silent_frames += 1;
        }

        // ── Partial flush (speech too long, don't wait for silence) ──────
        if accum.samples.len() >= PARTIAL_FLUSH_SAMPLES {
            // Keep the last PARTIAL_OVERLAP_SAMPLES as a running pre-roll so
            // the *next* chunk has Whisper encoder context and avoids cut-word
            // artefacts at the chunk boundary.
            let overlap_start = accum.samples.len().saturating_sub(PARTIAL_OVERLAP_SAMPLES);
            let overlap: Vec<f32> = accum.samples[overlap_start..].to_vec();
            let samples = {
                let mut s = std::mem::replace(&mut accum.samples, overlap);
                // The overlap is already in `accum.samples`; make sure we only
                // emit the non-overlap portion.
                let emit_len = s.len().saturating_sub(PARTIAL_OVERLAP_SAMPLES);
                s.truncate(emit_len + PARTIAL_OVERLAP_SAMPLES);
                s
            };
            let started_at_ms = accum.started_at_ms;
            let uid = accum.utterance_id;
            // Advance the start time for the continuing segment.
            accum.started_at_ms = now_ms
                .saturating_sub((PARTIAL_OVERLAP_SAMPLES * 1000 / SAMPLE_RATE) as u64);
            accum.silent_frames = 0;
            log::info!(
                "VAD partial flush [{}]: {} samples ({:.2}s)  [{:.3}s – {:.3}s]",
                uid,
                samples.len(),
                samples.len() as f64 / SAMPLE_RATE as f64,
                started_at_ms as f64 / 1000.0,
                now_ms as f64 / 1000.0,
            );
            return Some(AudioChunk {
                samples,
                started_at_ms,
                ended_at_ms: now_ms,
                utterance_id: uid,
                is_partial: true,
            });
        }

        // ── End of utterance (silence or hard cap) ─────────────────────
        let ended_by_silence = accum.silent_frames >= SILENCE_FRAMES;
        let ended_by_max = accum.samples.len() >= MAX_CHUNK_SAMPLES;

        if ended_by_silence || ended_by_max {
            let accum = speech.take().unwrap();
            log::info!(
                "VAD chunk [{}] ({}): {} samples ({:.2}s)  [{:.3}s – {:.3}s]  threshold={:.1}dBFS",
                accum.utterance_id,
                if ended_by_max { "max-length" } else { "silence" },
                accum.samples.len(),
                accum.samples.len() as f64 / SAMPLE_RATE as f64,
                accum.started_at_ms as f64 / 1000.0,
                now_ms as f64 / 1000.0,
                20.0_f32 * threshold.log10(),
            );
            preroll.push_slice(frame);
            return Some(AudioChunk {
                samples: accum.samples,
                started_at_ms: accum.started_at_ms,
                ended_at_ms: now_ms,
                utterance_id: accum.utterance_id,
                is_partial: false,
            });
        }
    } else {
        // No active speech segment yet — onset detection phase.
        if is_speech {
            *onset_count += 1;
            // Buffer the frame in preroll regardless; we'll include it in the
            // utterance if onset is confirmed, or discard it as a transient.
            preroll.push_slice(frame);

            if *onset_count >= SPEECH_ONSET_FRAMES {
                // Confirmed speech onset — start a new utterance.
                *onset_count = 0;
                *utterance_counter += 1;
                let uid = *utterance_counter;
                let preroll_ms = (PREROLL_SAMPLES * 1000 / SAMPLE_RATE) as u64;
                let started_at_ms = now_ms.saturating_sub(preroll_ms);
                let mut samples = Vec::with_capacity(PREROLL_SAMPLES + FRAME_SAMPLES);
                preroll.read_last(PREROLL_SAMPLES, &mut samples);
                log::info!(
                    "VAD speech start [{}] at {:.3}s  rms={:.4} ({:.1} dBFS)  threshold={:.1} dBFS",
                    uid,
                    now_ms as f64 / 1000.0,
                    frame_rms,
                    20.0_f32 * frame_rms.log10(),
                    20.0_f32 * threshold.log10(),
                );
                *speech = Some(SpeechAccum {
                    samples,
                    started_at_ms,
                    silent_frames: 0,
                    utterance_id: uid,
                });
            }
        } else {
            // Below threshold: reset onset counter and keep buffering in preroll.
            *onset_count = 0;
            preroll.push_slice(frame);
        }
    }

    None
}
