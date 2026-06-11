//! WASAPI loopback capture (Windows only, shared mode, ADR-0002).
//! Spawns a capture thread, a VAD worker, and an ASR worker.
//! All three share a stop flag and communicate through bounded channels.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::AppHandle;
use wasapi::{Direction, ShareMode, get_default_device};

use crate::asr;
use crate::audio::resample::Resampler16k;
use crate::pipeline;
use crate::state::{self, AppState};
use crate::translate;
use crate::types::AudioProcess;

/// Port asr-srv listens on.  Configurable via env `ASR_PORT` (or legacy `WHISPER_ASR_PORT`).
const DEFAULT_ASR_PORT: u16 = 9001;
/// Port llama-server listens on.  Configurable via env `LLAMA_PORT`.
const DEFAULT_LLAMA_PORT: u16 = 9002;

/// Spawn the full audio pipeline: WASAPI capture → VAD → ASR → Translation.
/// All workers exit cleanly when `stop` is set to `true`.
pub fn start_loopback_capture(app: AppHandle, stop: Arc<AtomicBool>) {
    let asr_port = std::env::var("ASR_PORT")
        .or_else(|_| std::env::var("WHISPER_ASR_PORT")) // legacy compat
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_ASR_PORT);

    let llama_port = std::env::var("LLAMA_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_LLAMA_PORT);

    // ASR → Translation: the worker drains the backlog and translates only the
    // newest request, so a slightly larger buffer just smooths bursts.
    let (tl_tx, tl_rx) = mpsc::sync_channel::<translate::TranslationRequest>(4);
    // VAD → ASR: the worker coalesces its backlog (stale partials are skipped),
    // and the chunker blocks on finals — 8 slots absorb bursts without loss.
    let (asr_tx, asr_rx) = mpsc::sync_channel::<asr::AudioChunk>(8);
    // Capture → VAD: unbounded is fine — VAD is fast (just RMS).
    let (vad_tx, vad_rx) = mpsc::channel::<Vec<f32>>();

    // Read current settings once at pipeline start.
    let (speech_threshold, music_mode_flag, capture_pid, capture_name) =
        state::read_state(&app, |s| {
            let (pid, name) = s.capture_target.as_ref()
                .map(|p| (p.pid, p.name.clone()))
                .unwrap_or((0, String::new()));
            (s.speech_threshold, Arc::clone(&s.music_mode_flag), pid, name)
        })
        .unwrap_or_else(|| (0.032, Arc::new(AtomicBool::new(false)), 0, String::new()));

    // Clear any stale loopback error from a previous session.
    state::update_and_emit(&app, |s| s.loopback_error = None);

    log::info!(
        "pipeline start: music_mode={}",
        music_mode_flag.load(Ordering::Relaxed),
    );

    translate::llama_server::start_translate_worker(tl_rx, app.clone(), llama_port, Arc::clone(&stop));
    asr::http_client::start_asr_worker(asr_rx, app.clone(), asr_port, Arc::clone(&stop), tl_tx);
    pipeline::chunker::start_vad_worker(vad_rx, asr_tx, Arc::clone(&stop), speech_threshold, music_mode_flag);

    std::thread::Builder::new()
        .name("wasapi-loopback".into())
        .spawn(move || {
            let result = if capture_pid == 0 {
                capture_loop(&app, &stop, vad_tx)
            } else {
                run_process_capture(&app, &stop, vad_tx, capture_pid, &capture_name)
            };
            if let Err(e) = result {
                log::error!("capture error: {e}");
            }
            log::info!("capture thread exited");
        })
        .expect("spawn wasapi-loopback thread");
}

/// Process-targeted capture with PID refresh, one recovery retry, and
/// system-loopback fallback.
fn run_process_capture(
    app: &AppHandle,
    stop: &Arc<AtomicBool>,
    vad_tx: mpsc::Sender<Vec<f32>>,
    stored_pid: u32,
    name: &str,
) -> Result<(), String> {
    // Refresh the PID right before activation — Chromium browsers recycle
    // their audio renderer subprocess PID, so the one from the picker may be
    // stale by the time the user hits Start.
    let pid = find_audio_pid(name, None).unwrap_or(stored_pid);
    if pid != stored_pid {
        log::info!("PID refreshed: {stored_pid} → {pid} for {name}");
        set_capture_target(app, pid, name);
    }

    log::info!("Using process loopback for PID {pid} ({name})");
    let err = match super::process_loopback::run_process_loopback(app, stop, vad_tx.clone(), pid) {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    log::warn!("Process loopback PID {pid} ({name}) failed: {err}");

    // Second chance: re-enumerate in case the renderer cycled during the
    // brief window between refresh and activate.
    if let Some(new_pid) = find_audio_pid(name, Some(pid)) {
        log::info!("Auto-recovery: retrying with PID {new_pid} for {name}");
        set_capture_target(app, new_pid, name);
        match super::process_loopback::run_process_loopback(app, stop, vad_tx.clone(), new_pid) {
            Ok(()) => return Ok(()),
            Err(e2) => {
                log::warn!("Recovery loopback PID {new_pid} also failed: {e2}");
                clear_capture_target(app, Some(e2));
            }
        }
    } else {
        log::warn!("No active audio session found for {name:?}, falling back to system loopback");
        clear_capture_target(app, Some(err));
    }
    capture_loop(app, stop, vad_tx)
}

/// Clear `capture_target` in AppState and store the loopback error for UI display.
fn clear_capture_target(app: &AppHandle, error: Option<String>) {
    state::update_and_emit(app, |s| {
        s.capture_target = None;
        s.loopback_error = error;
    });
}

fn set_capture_target(app: &AppHandle, pid: u32, name: &str) {
    let name = name.to_string();
    state::update_and_emit(app, |s| {
        s.capture_target = Some(AudioProcess { pid, name });
    });
}

/// Enumerate active audio sessions on a fresh thread (avoids COM apartment
/// conflicts with the capture thread's MTA) and return the PID of the first
/// session owned by `app_name`, optionally excluding a known-stale PID.
fn find_audio_pid(app_name: &str, exclude: Option<u32>) -> Option<u32> {
    if app_name.is_empty() {
        return None;
    }
    let name = app_name.to_string();
    std::thread::spawn(move || {
        wasapi::initialize_mta().ok();
        super::session_enum::list_audio_processes()
            .unwrap_or_default()
            .into_iter()
            .find(|p| Some(p.pid) != exclude && p.name.eq_ignore_ascii_case(&name))
            .map(|p| p.pid)
    })
    .join()
    .ok()
    .flatten()
}

// ── shared audio plumbing ────────────────────────────────────────────────────

/// Common per-capture state: raw byte queue → f32 samples → (every ~200 ms)
/// RMS to the UI + resampled 16 kHz audio to the VAD worker.
/// Shared by the system-wide loop below and `process_loopback`.
pub struct AudioPump {
    /// Raw PCM bytes from the device; public so wasapi can append directly.
    pub byte_queue: VecDeque<u8>,
    f32_buf: Vec<f32>,
    last_emit: Instant,
    block_align: usize,
    bits_per_sample: u16,
    channels: usize,
    resampler: Resampler16k,
}

impl AudioPump {
    pub fn new(
        sample_rate: u32,
        channels: usize,
        bits_per_sample: u16,
        block_align: usize,
    ) -> Result<Self, String> {
        Ok(AudioPump {
            byte_queue: VecDeque::new(),
            f32_buf: Vec::new(),
            last_emit: Instant::now(),
            block_align,
            bits_per_sample,
            channels,
            resampler: Resampler16k::new(sample_rate, channels)?,
        })
    }

    /// Convert all complete frames in `byte_queue` to f32 samples.
    /// Bulk drain: one allocation per wakeup instead of one per audio frame.
    pub fn drain_frames(&mut self) {
        let n = self.byte_queue.len() / self.block_align * self.block_align;
        if n == 0 {
            return;
        }
        let bytes: Vec<u8> = self.byte_queue.drain(..n).collect();
        self.f32_buf.extend_from_slice(&bytes_to_f32(&bytes, self.bits_per_sample));
    }

    /// Every ~200 ms: emit RMS to the UI and forward resampled audio to VAD.
    pub fn tick(&mut self, app: &AppHandle, vad_tx: &mpsc::Sender<Vec<f32>>) {
        if self.f32_buf.is_empty() || self.last_emit.elapsed() < Duration::from_millis(200) {
            return;
        }
        self.last_emit = Instant::now();

        let mono = interleaved_to_mono(&self.f32_buf, self.channels);
        let rms = super::meter::rms(&mono);
        state::update_and_emit(app, |s: &mut AppState| s.rms = rms);

        match self.resampler.process(&self.f32_buf) {
            Ok(samples_16k) if !samples_16k.is_empty() => {
                let _ = vad_tx.send(samples_16k);
            }
            Ok(_) => {}
            Err(e) => log::warn!("resample error: {e}"),
        }
        self.f32_buf.clear();
    }
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

    let mut pump = AudioPump::new(sample_rate, channels, bits_per_sample, block_align)?;

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
                        .read_from_device_to_deque(block_align, &mut pump.byte_queue)
                        .map_err(|e| e.to_string())?;
                }
            }
        }

        pump.drain_frames();
        pump.tick(app, &vad_tx);
    }

    audio_client.stop_stream().map_err(|e| e.to_string())?;
    log::info!("WASAPI loopback stream stopped");
    Ok(())
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
pub fn interleaved_to_mono(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels == 1 {
        return samples.to_vec();
    }
    samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}
