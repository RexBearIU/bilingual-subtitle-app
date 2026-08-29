//! Record system output audio (WASAPI loopback) to a 16 kHz mono WAV.
//!
//! Captures whatever is playing through the default output device — the same
//! signal the app's pipeline sees, including browser/stream processing — so
//! `bench/compare_backends.py` measures the real thing rather than a pristine
//! source file.  16 kHz mono is also exactly the format the bench script
//! decodes natively, so no ffmpeg is needed anywhere in the loop.
//!
//! Reuses `audio::capture::bytes_to_f32` and `audio::resample::Resampler16k`
//! rather than reimplementing them, so the recording path matches the live one.
//!
//! Usage:
//!     cargo run --bin record_loopback -- --seconds 60 --out ../bench/sample.wav
//!
//! The WAV header is rewritten every second, so killing the process with
//! Ctrl+C still leaves a playable file containing everything captured so far.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use app_lib::audio::capture::bytes_to_f32;
use app_lib::audio::resample::Resampler16k;
use wasapi::{Direction, ShareMode, get_default_device};

const OUT_RATE: u32 = 16_000;
const HEADER_BYTES: u64 = 44;

struct Args {
    seconds: f64,
    out: PathBuf,
}

fn parse_args() -> Result<Args, String> {
    let mut seconds = 60.0_f64;
    let mut out = PathBuf::from("recording.wav");
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--seconds" | "-s" => {
                seconds = it
                    .next()
                    .ok_or("--seconds needs a value")?
                    .parse()
                    .map_err(|e| format!("--seconds: {e}"))?;
            }
            "--out" | "-o" => out = PathBuf::from(it.next().ok_or("--out needs a value")?),
            "--help" | "-h" => {
                println!(
                    "record system audio to a 16 kHz mono WAV\n\n\
                     \x20 --seconds, -s   duration in seconds (default 60)\n\
                     \x20 --out, -o       output path (default recording.wav)"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other:?} (try --help)")),
        }
    }
    // Spelled out rather than negating `>` so the NaN case is explicit.
    if seconds.is_nan() || seconds <= 0.0 {
        return Err("--seconds must be a positive number".into());
    }
    Ok(Args { seconds, out })
}

/// Write a 16-bit mono PCM header with zeroed sizes; patched by `patch_sizes`.
fn write_placeholder_header(w: &mut BufWriter<File>) -> std::io::Result<()> {
    let byte_rate = OUT_RATE * 2; // mono * 2 bytes
    w.write_all(b"RIFF")?;
    w.write_all(&0u32.to_le_bytes())?; // patched: 36 + data_len
    w.write_all(b"WAVEfmt ")?;
    w.write_all(&16u32.to_le_bytes())?; // PCM fmt chunk size
    w.write_all(&1u16.to_le_bytes())?; // PCM
    w.write_all(&1u16.to_le_bytes())?; // mono
    w.write_all(&OUT_RATE.to_le_bytes())?;
    w.write_all(&byte_rate.to_le_bytes())?;
    w.write_all(&2u16.to_le_bytes())?; // block align
    w.write_all(&16u16.to_le_bytes())?; // bits per sample
    w.write_all(b"data")?;
    w.write_all(&0u32.to_le_bytes())?; // patched: data_len
    Ok(())
}

/// Rewrite the two length fields, then seek back to the end to keep appending.
fn patch_sizes(w: &mut BufWriter<File>, data_len: u32) -> std::io::Result<()> {
    w.flush()?;
    w.seek(SeekFrom::Start(4))?;
    w.write_all(&(36 + data_len).to_le_bytes())?;
    w.seek(SeekFrom::Start(40))?;
    w.write_all(&data_len.to_le_bytes())?;
    w.flush()?;
    w.seek(SeekFrom::Start(HEADER_BYTES + data_len as u64))?;
    Ok(())
}

fn main() -> Result<(), String> {
    let args = parse_args()?;

    wasapi::initialize_mta().map_err(|e| e.to_string())?;

    let device = get_default_device(&Direction::Render).map_err(|e| e.to_string())?;
    let mut audio_client = device.get_iaudioclient().map_err(|e| e.to_string())?;
    let format = audio_client.get_mixformat().map_err(|e| e.to_string())?;

    let sample_rate = format.get_samplespersec();
    let channels = format.get_nchannels() as usize;
    let bits_per_sample = format.get_bitspersample();
    let block_align = format.get_blockalign() as usize;

    println!(
        "device: {sample_rate} Hz  {channels} ch  {bits_per_sample} bps  → {OUT_RATE} Hz mono"
    );

    let mut resampler = Resampler16k::new(sample_rate, channels)?;

    let (default_period, _) = audio_client.get_periods().map_err(|e| e.to_string())?;
    audio_client
        .initialize_client(
            &format,
            default_period,
            &Direction::Capture,
            &ShareMode::Shared,
            true, // loopback
        )
        .map_err(|e| e.to_string())?;

    let h_event = audio_client.set_get_eventhandle().map_err(|e| e.to_string())?;
    let capture_client = audio_client.get_audiocaptureclient().map_err(|e| e.to_string())?;

    let file = File::create(&args.out).map_err(|e| format!("create {:?}: {e}", args.out))?;
    let mut w = BufWriter::new(file);
    write_placeholder_header(&mut w).map_err(|e| e.to_string())?;

    audio_client.start_stream().map_err(|e| e.to_string())?;
    println!(
        "recording {:.0}s → {}   (Ctrl+C stops early; the file stays valid)",
        args.seconds,
        args.out.display()
    );

    let mut queue = std::collections::VecDeque::<u8>::new();
    let mut data_len: u32 = 0;
    let mut peak = 0.0f32;
    let started = Instant::now();
    let total = Duration::from_secs_f64(args.seconds);
    let mut last_patch = Instant::now();
    let mut last_report = Instant::now();

    while started.elapsed() < total {
        if h_event.wait_for_event(200).is_err() {
            continue; // no data this period
        }
        loop {
            match capture_client.get_next_nbr_frames().map_err(|e| e.to_string())? {
                Some(0) | None => break,
                Some(_) => {
                    // Returns BufferFlags (silence/discontinuity); not needed here.
                    capture_client
                        .read_from_device_to_deque(block_align, &mut queue)
                        .map_err(|e| e.to_string())?;
                }
            }
        }

        // Take whole frames only; a partial frame waits for the next period.
        let usable = queue.len() - (queue.len() % block_align);
        if usable == 0 {
            continue;
        }
        let bytes: Vec<u8> = queue.drain(..usable).collect();
        let interleaved = bytes_to_f32(&bytes, bits_per_sample);

        for s in resampler.process(&interleaved)? {
            peak = peak.max(s.abs());
            let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            w.write_all(&v.to_le_bytes()).map_err(|e| e.to_string())?;
            data_len += 2;
        }

        if last_patch.elapsed() >= Duration::from_secs(1) {
            patch_sizes(&mut w, data_len).map_err(|e| e.to_string())?;
            last_patch = Instant::now();
        }
        if last_report.elapsed() >= Duration::from_secs(5) {
            println!(
                "  {:.0}s  peak {:.0}%",
                started.elapsed().as_secs_f64(),
                peak * 100.0
            );
            peak = 0.0;
            last_report = Instant::now();
        }
    }

    audio_client.stop_stream().map_err(|e| e.to_string())?;
    patch_sizes(&mut w, data_len).map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())?;

    let secs = data_len as f64 / 2.0 / OUT_RATE as f64;
    println!("wrote {} — {secs:.1}s, {} KB", args.out.display(), data_len / 1024);
    if data_len == 0 {
        println!("NOTE: captured nothing. Is audio actually playing on the default output device?");
    }
    Ok(())
}
