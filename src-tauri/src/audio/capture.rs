//! WASAPI loopback capture (Windows only, shared mode, ADR-0002).
//! Spawns a detached thread that reads from the default render endpoint in
//! loopback mode, computes RMS, and emits `engine_status` every ~200 ms.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};
use wasapi::{Direction, ShareMode, get_default_device};

use crate::state::AppState;
use crate::types::EngineStatus;

/// Spawn the WASAPI loopback capture thread (detached).
/// The thread exits cleanly when `stop` is set to `true`.
pub fn start_loopback_capture(app: AppHandle, stop: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("wasapi-loopback".into())
        .spawn(move || {
            if let Err(e) = capture_loop(&app, &stop) {
                log::error!("WASAPI capture error: {e}");
            }
            log::info!("WASAPI capture thread exited");
        })
        .expect("spawn wasapi-loopback thread");
}

fn capture_loop(app: &AppHandle, stop: &Arc<AtomicBool>) -> Result<(), String> {
    // Initialize COM for this thread (MTA, safe on a new non-UI thread).
    wasapi::initialize_mta().map_err(|e| e.to_string())?;

    // Loopback capture: use the *Render* endpoint but ask WASAPI to capture
    // what it's playing (AUDCLNT_STREAMFLAGS_LOOPBACK, set by the crate when
    // device direction = Render and initialize direction = Capture).
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

    let (default_period, _) = audio_client.get_periods().map_err(|e| e.to_string())?;

    // Render device + Capture direction → crate sets AUDCLNT_STREAMFLAGS_LOOPBACK.
    // convert=true enables AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM so the mix
    // format is always accepted without manual format negotiation.
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
        // Wait up to 100 ms; Err means timeout (no data), which is fine —
        // we just loop back and check the stop flag again.
        match h_event.wait_for_event(100) {
            Ok(()) => {}
            Err(_) => continue,
        }

        // Drain every pending WASAPI packet.
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

        // Convert raw bytes → interleaved f32.
        while byte_queue.len() >= block_align {
            let frame_bytes: Vec<u8> = byte_queue.drain(..block_align).collect();
            f32_buf.extend_from_slice(&bytes_to_f32(&frame_bytes, bits_per_sample));
        }

        // Emit RMS every ~200 ms; skip if we've been asked to stop.
        if last_emit.elapsed() >= Duration::from_millis(200)
            && !f32_buf.is_empty()
            && !stop.load(Ordering::Relaxed)
        {
            let mono = interleaved_to_mono(&f32_buf, channels);
            let rms = super::meter::rms(&mono);
            f32_buf.clear();
            last_emit = Instant::now();

            log::info!(
                "RMS {:.5} ({:.1} dBFS)",
                rms,
                super::meter::rms_to_dbfs(rms)
            );

            push_rms(app, rms);
        }
    }

    audio_client.stop_stream().map_err(|e| e.to_string())?;
    log::info!("WASAPI loopback stream stopped");
    Ok(())
}

/// Write the latest RMS into AppState and re-emit `engine_status`.
fn push_rms(app: &AppHandle, rms: f32) {
    if let Some(st) = app.try_state::<Mutex<AppState>>() {
        if let Ok(mut s) = st.lock() {
            s.rms = rms;
            let status = EngineStatus::from_state(&s);
            let _ = app.emit("engine_status", status);
        }
    }
}

/// Convert a raw LE byte frame to f32 samples.
fn bytes_to_f32(bytes: &[u8], bits_per_sample: u16) -> Vec<f32> {
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
                // Sign-extend 24-bit LE integer to 32-bit.
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

/// Average interleaved multi-channel samples to a single mono channel.
fn interleaved_to_mono(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels == 1 {
        return samples.to_vec();
    }
    samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}
