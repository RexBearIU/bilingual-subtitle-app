//! WASAPI loopback capture (Windows only, shared mode, ADR-0002).
//! Spawns a capture thread, a VAD worker, and an ASR worker.
//! All three share a stop flag and communicate through bounded channels.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};
use wasapi::{Direction, ShareMode, get_default_device};

use crate::asr;
use crate::audio::resample::Resampler16k;
use crate::pipeline;
use crate::state::AppState;
use crate::translate;
use crate::types::EngineStatus;

/// Port whisper-server listens on.  Configurable via env `WHISPER_ASR_PORT`.
const DEFAULT_ASR_PORT: u16 = 9001;
/// Port llama-server listens on.  Configurable via env `LLAMA_PORT`.
const DEFAULT_LLAMA_PORT: u16 = 9002;

/// Spawn the full audio pipeline: WASAPI capture → VAD → ASR → Translation.
/// All workers exit cleanly when `stop` is set to `true`.
pub fn start_loopback_capture(app: AppHandle, stop: Arc<AtomicBool>) {
    let asr_port = std::env::var("WHISPER_ASR_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_ASR_PORT);

    let llama_port = std::env::var("LLAMA_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_LLAMA_PORT);

    // ASR → Translation: bounded to 2 — drop translation requests rather than
    // accumulate stale ones if the translation worker falls behind.
    let (tl_tx, tl_rx) = mpsc::sync_channel::<translate::TranslationRequest>(2);
    // VAD → ASR: bounded to 2 — drop audio chunks rather than queue indefinitely
    // if ASR falls behind (e.g. long whisper inference).
    let (asr_tx, asr_rx) = mpsc::sync_channel::<asr::AudioChunk>(2);
    // Capture → VAD: unbounded is fine — VAD is fast (just RMS).
    let (vad_tx, vad_rx) = mpsc::channel::<Vec<f32>>();

    // Read current settings once at pipeline start.
    let (speech_threshold, music_mode_flag, capture_pid) = app
        .try_state::<Mutex<AppState>>()
        .and_then(|st| st.lock().ok().map(|s| {
            (
                s.speech_threshold,
                Arc::clone(&s.music_mode_flag),
                s.capture_target.as_ref().map(|p| p.pid).unwrap_or(0),
            )
        }))
        .unwrap_or_else(|| (0.032, Arc::new(AtomicBool::new(false)), 0));

    log::info!(
        "pipeline start: speech_threshold={speech_threshold:.4} ({:.1} dBFS)  music_mode={}",
        20.0_f32 * speech_threshold.log10(),
        music_mode_flag.load(Ordering::Relaxed),
    );

    translate::llama_server::start_translate_worker(tl_rx, app.clone(), llama_port, Arc::clone(&stop));
    asr::whisper_server::start_asr_worker(asr_rx, app.clone(), asr_port, Arc::clone(&stop), tl_tx);
    pipeline::vad::start_vad_worker(vad_rx, asr_tx, Arc::clone(&stop), speech_threshold, music_mode_flag);

    std::thread::Builder::new()
        .name("wasapi-loopback".into())
        .spawn(move || {
            let result = if capture_pid == 0 {
                capture_loop(&app, &stop, vad_tx)
            } else {
                log::info!("Using process loopback for PID {capture_pid}");
                super::process_loopback::run_process_loopback(&app, &stop, vad_tx, capture_pid)
            };
            if let Err(e) = result {
                log::error!("capture error: {e}");
            }
            log::info!("capture thread exited");
        })
        .expect("spawn wasapi-loopback thread");
}

fn capture_loop(
    app: &AppHandle,
    stop: &Arc<AtomicBool>,
    vad_tx: mpsc::Sender<Vec<f32>>,
) -> Result<(), String> {
    wasapi::initialize_mta().map_err(|e| e.to_string())?;

    let device = get_default_device(&Direction::Render).map_err(|e| e.to_string())?;
    let mut audio_client = device.get_iaudioclient().map_err(|e| e.to_string())?;
    let format = audio_client.get_mixformat().map_err(|e| e.to_string())?;

    let sample_rate = format.get_samplespersec();
    let channels = format.get_nchannels() as usize;
    let bits_per_sample = format.get_bitspersample();
    let block_align = format.get_blockalign() as usize;

    log::info!(
        "WASAPI loopback: {} Hz  {} ch  {} bps  block_align={}",
        sample_rate, channels, bits_per_sample, block_align
    );

    let mut resampler = Resampler16k::new(sample_rate, channels)?;

    let (default_period, _) = audio_client.get_periods().map_err(|e| e.to_string())?;

    audio_client
        .initialize_client(
            &format,
            default_period,
            &Direction::Capture,
            &ShareMode::Shared,
            true,
        )
        .map_err(|e| e.to_string())?;

    let h_event = audio_client
        .set_get_eventhandle()
        .map_err(|e| e.to_string())?;
    let capture_client = audio_client
        .get_audiocaptureclient()
        .map_err(|e| e.to_string())?;
    audio_client.start_stream().map_err(|e| e.to_string())?;

    log::info!("WASAPI loopback stream started");

    let mut byte_queue: VecDeque<u8> = VecDeque::new();
    let mut f32_buf: Vec<f32> = Vec::new();
    let mut last_emit = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        match h_event.wait_for_event(100) {
            Ok(()) => {}
            Err(_) => continue,
        }

        loop {
            match capture_client.get_next_nbr_frames().map_err(|e| e.to_string())? {
                Some(0) | None => break,
                Some(_) => {
                    capture_client
                        .read_from_device_to_deque(block_align, &mut byte_queue)
                        .map_err(|e| e.to_string())?;
                }
            }
        }

        while byte_queue.len() >= block_align {
            let frame_bytes: Vec<u8> = byte_queue.drain(..block_align).collect();
            f32_buf.extend_from_slice(&bytes_to_f32(&frame_bytes, bits_per_sample));
        }

        // Every ~200 ms: emit RMS to UI and forward resampled audio to VAD.
        if last_emit.elapsed() >= Duration::from_millis(200)
            && !f32_buf.is_empty()
            && !stop.load(Ordering::Relaxed)
        {
            let mono = interleaved_to_mono(&f32_buf, channels);
            let rms = super::meter::rms(&mono);
            last_emit = Instant::now();

            log::debug!("RMS {:.5} ({:.1} dBFS)", rms, super::meter::rms_to_dbfs(rms));
            push_rms(app, rms);

            match resampler.process(&f32_buf) {
                Ok(samples_16k) if !samples_16k.is_empty() => {
                    let _ = vad_tx.send(samples_16k);
                }
                Ok(_) => {}
                Err(e) => log::warn!("resample error: {e}"),
            }

            f32_buf.clear();
        }
    }

    audio_client.stop_stream().map_err(|e| e.to_string())?;
    log::info!("WASAPI loopback stream stopped");
    Ok(())
}

fn push_rms(app: &AppHandle, rms: f32) {
    if let Some(st) = app.try_state::<Mutex<AppState>>() {
        if let Ok(mut s) = st.lock() {
            s.rms = rms;
            let status = EngineStatus::from_state(&s);
            let _ = app.emit("engine_status", status);
        }
    }
}

/// Convert raw PCM bytes to normalised f32 samples.
/// Shared with `process_loopback`.
pub fn bytes_to_f32(bytes: &[u8], bits_per_sample: u16) -> Vec<f32> {
    match bits_per_sample {
        32 => bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect(),
        16 => bytes
            .chunks_exact(2)
            .map(|b| {
                let v = i16::from_le_bytes([b[0], b[1]]);
                v as f32 / 32_768.0
            })
            .collect(),
        24 => bytes
            .chunks_exact(3)
            .map(|b| {
                let raw = u32::from_le_bytes([b[0], b[1], b[2], 0]) as i32;
                let signed = (raw << 8) >> 8;
                signed as f32 / 8_388_608.0
            })
            .collect(),
        _ => {
            log::warn!("unsupported audio sample width {} bps — skipping frame", bits_per_sample);
            vec![]
        }
    }
}

/// Mix interleaved multi-channel samples down to mono.
/// Shared with `process_loopback`.
pub fn interleaved_to_mono(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels == 1 {
        return samples.to_vec();
    }
    samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}
