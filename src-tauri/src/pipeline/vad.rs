//! RMS VAD v1 — gates audio chunks for ASR (M3).
//!
//! Receives 16 kHz mono f32 samples from the capture thread via channel,
//! processes them in 25 ms frames, and emits chunks whenever a speech
//! segment ends (silence timeout or max-length limit).
//!
//! M4 will wire `emit_chunk` to the ASR worker.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Instant;

use crate::audio::meter::rms;
use crate::audio::ring_buffer::RingBuffer;

/// 16 kHz frames of 25 ms each.
const SAMPLE_RATE: usize = 16_000;
const FRAME_SAMPLES: usize = 400;
/// 300 ms pre-roll — included at the start of every chunk.
const PREROLL_SAMPLES: usize = 4_800;
/// 20 silent frames (500 ms) → end of speech segment.
const SILENCE_FRAMES: usize = 20;
/// 8 s hard cap on a single chunk.
const MAX_CHUNK_SAMPLES: usize = 128_000;
/// RMS threshold for speech (~−46 dBFS).
pub const SPEECH_THRESHOLD: f32 = 0.005;

struct SpeechAccum {
    samples: Vec<f32>,
    /// Session-relative start time (ms), adjusted for pre-roll.
    started_at_ms: u64,
    /// Consecutive silent frames since last voiced frame.
    silent_frames: usize,
}

/// Spawn the VAD worker thread (detached).  Exits when `stop` is set or the
/// sender side of `rx` is dropped.
pub fn start_vad_worker(rx: Receiver<Vec<f32>>, stop: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("vad-worker".into())
        .spawn(move || vad_loop(rx, &stop))
        .expect("spawn vad-worker thread");
}

fn vad_loop(rx: Receiver<Vec<f32>>, stop: &Arc<AtomicBool>) {
    let mut preroll = RingBuffer::new(PREROLL_SAMPLES);
    let mut pending: Vec<f32> = Vec::new();
    let mut speech: Option<SpeechAccum> = None;
    let session_start = Instant::now();

    loop {
        if stop.load(Ordering::Relaxed) {
            if let Some(accum) = speech.take() {
                if accum.samples.len() > FRAME_SAMPLES {
                    let ended_at_ms = session_start.elapsed().as_millis() as u64;
                    log::info!(
                        "VAD flush on stop: {} samples  [{:.3}s – {:.3}s]",
                        accum.samples.len(),
                        accum.started_at_ms as f64 / 1000.0,
                        ended_at_ms as f64 / 1000.0
                    );
                    emit_chunk(accum.samples, accum.started_at_ms, ended_at_ms);
                }
            }
            break;
        }

        let chunk = match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(c) => c,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };

        pending.extend_from_slice(&chunk);

        while pending.len() >= FRAME_SAMPLES {
            let frame: Vec<f32> = pending.drain(..FRAME_SAMPLES).collect();
            process_frame(&frame, &mut speech, &mut preroll, &session_start);
        }
    }

    log::info!("VAD worker exited");
}

fn process_frame(
    frame: &[f32],
    speech: &mut Option<SpeechAccum>,
    preroll: &mut RingBuffer,
    session_start: &Instant,
) {
    let frame_rms = rms(frame);
    let is_speech = frame_rms >= SPEECH_THRESHOLD;
    let now_ms = session_start.elapsed().as_millis() as u64;

    // Accumulate frame into ongoing speech segment and check termination conditions.
    let should_end = if let Some(ref mut accum) = speech {
        accum.samples.extend_from_slice(frame);
        if is_speech {
            accum.silent_frames = 0;
        } else {
            accum.silent_frames += 1;
        }
        accum.silent_frames >= SILENCE_FRAMES || accum.samples.len() >= MAX_CHUNK_SAMPLES
    } else {
        false
    };

    if should_end {
        let is_max = speech
            .as_ref()
            .map(|a| a.samples.len() >= MAX_CHUNK_SAMPLES)
            .unwrap_or(false);
        let reason = if is_max { "max-length" } else { "silence-timeout" };
        let accum = speech.take().unwrap();
        log::info!(
            "VAD chunk [{}]: {} samples  {:.2}s  [{:.3}s – {:.3}s]",
            reason,
            accum.samples.len(),
            accum.samples.len() as f64 / SAMPLE_RATE as f64,
            accum.started_at_ms as f64 / 1000.0,
            now_ms as f64 / 1000.0,
        );
        emit_chunk(accum.samples, accum.started_at_ms, now_ms);
        // Seed pre-roll with the trailing silent frame.
        preroll.push_slice(frame);
    } else if speech.is_none() {
        if is_speech {
            // Start a new speech segment, prepending the pre-roll buffer.
            let preroll_ms = (PREROLL_SAMPLES * 1000 / SAMPLE_RATE) as u64;
            let started_at_ms = now_ms.saturating_sub(preroll_ms);
            let mut samples = Vec::with_capacity(PREROLL_SAMPLES + FRAME_SAMPLES);
            preroll.read_last(PREROLL_SAMPLES, &mut samples);
            samples.extend_from_slice(frame);
            log::info!(
                "VAD speech start at {:.3}s (rms={:.4}  {:.1} dBFS)",
                now_ms as f64 / 1000.0,
                frame_rms,
                crate::audio::meter::rms_to_dbfs(frame_rms),
            );
            *speech = Some(SpeechAccum { samples, started_at_ms, silent_frames: 0 });
        } else {
            preroll.push_slice(frame);
        }
    }
    // If speech.is_some() && !should_end: still accumulating — nothing more to do.
}

/// Emit a completed audio chunk.  M4 will forward `samples` to the ASR worker.
fn emit_chunk(samples: Vec<f32>, started_at_ms: u64, ended_at_ms: u64) {
    // Silence the "unused variable" warning until M4 wires this to ASR.
    let _ = (samples, started_at_ms, ended_at_ms);
}
