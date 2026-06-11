//! Windows Process Loopback capture (Win 10 Build 20348 / Win 11+).
//!
//! Uses `ActivateAudioInterfaceAsync` with `AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`
//! to capture audio from a single process without needing a virtual audio device.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use tauri::AppHandle;
use windows::core::{implement, Interface, HRESULT};
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::Media::Audio::{
    ActivateAudioInterfaceAsync,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
    AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
    IAudioCaptureClient, IAudioClient,
    IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler,
    IActivateAudioInterfaceCompletionHandler_Impl,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
    VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
};
// CreateEventW needs Win32_Security feature (SECURITY_ATTRIBUTES parameter).
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use crate::audio::capture::AudioPump;

// ── COM completion handler ───────────────────────────────────────────────────

/// Sends the activated `IAudioClient` (or an error) back to the calling thread.
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct ActivationHandler {
    tx: mpsc::SyncSender<windows::core::Result<IAudioClient>>,
}

// SAFETY: IAudioClient is a reference-counted COM object and is Send.
// ActivationHandler is called from a Windows thread pool thread.
unsafe impl Send for ActivationHandler {}
unsafe impl Sync for ActivationHandler {}

impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationHandler_Impl {
    fn ActivateCompleted(
        &self,
        activateop: Option<&IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let result = (|| -> windows::core::Result<IAudioClient> {
            let op = activateop.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
            let mut hr = HRESULT(0);
            let mut activated: Option<windows::core::IUnknown> = None;
            unsafe { op.GetActivateResult(&mut hr, &mut activated)?; }
            hr.ok()?;
            activated
                .ok_or_else(|| windows::core::Error::from(E_FAIL))?
                .cast::<IAudioClient>()
        })();
        let _ = self.tx.try_send(result);
        Ok(())
    }
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Run the full capture loop for a single process.
/// `pid` must be a running process that is actively outputting audio.
/// Returns when `stop` is set or an unrecoverable error occurs.
pub fn run_process_loopback(
    app: &AppHandle,
    stop: &Arc<AtomicBool>,
    vad_tx: mpsc::Sender<Vec<f32>>,
    pid: u32,
) -> Result<(), String> {
    wasapi::initialize_mta().map_err(|e| e.to_string())?;

    let audio_client = activate_for_process(pid)?;

    // Get the device mix format.
    let format_ptr = unsafe {
        audio_client.GetMixFormat().map_err(|e| e.to_string())?
    };
    let (sample_rate, channels, bits_per_sample, block_align) = unsafe {
        let f = &*format_ptr;
        (
            f.nSamplesPerSec,
            f.nChannels as usize,
            f.wBitsPerSample,
            f.nBlockAlign as usize,
        )
    };
    // GetMixFormat allocates; free it after reading the fields.
    unsafe {
        windows::Win32::System::Com::CoTaskMemFree(Some(format_ptr as *mut _));
    }

    log::info!(
        "Process loopback (pid {pid}): {} Hz  {} ch  {} bps",
        sample_rate, channels, bits_per_sample
    );

    let mut pump = AudioPump::new(sample_rate, channels, bits_per_sample, block_align)?;

    // Re-query format for Initialize (the pointer was freed above).
    let format_ptr2 = unsafe {
        audio_client.GetMixFormat().map_err(|e| e.to_string())?
    };

    // Initialize: shared mode, loopback + event-driven.
    let flags = AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
    unsafe {
        audio_client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                flags,
                0, // let Windows choose buffer size
                0,
                format_ptr2,
                None,
            )
            .map_err(|e| format!("IAudioClient::Initialize: {e}"))?;
        windows::Win32::System::Com::CoTaskMemFree(Some(format_ptr2 as *mut _));
    }

    let h_event = unsafe {
        CreateEventW(None, false, false, None).map_err(|e| e.to_string())?
    };
    unsafe {
        audio_client
            .SetEventHandle(h_event)
            .map_err(|e| e.to_string())?;
    }

    let capture_client: IAudioCaptureClient = unsafe {
        audio_client.GetService().map_err(|e| e.to_string())?
    };

    unsafe { audio_client.Start().map_err(|e| e.to_string())? };
    log::info!("Process loopback stream started (pid {pid})");

    while !stop.load(Ordering::Relaxed) {
        let wait_result = unsafe { WaitForSingleObject(h_event, 100) };
        // WAIT_TIMEOUT (258) — normal, just loop back and check stop flag.
        if wait_result.0 != 0 {
            continue;
        }

        // Drain all available packets.
        loop {
            let packet_len = unsafe {
                match capture_client.GetNextPacketSize() {
                    Ok(n) => n,
                    Err(e) => {
                        log::warn!("GetNextPacketSize: {e}");
                        break;
                    }
                }
            };
            if packet_len == 0 {
                break;
            }

            // In windows 0.58, GetBuffer takes out-params (not returning a tuple).
            let mut data_ptr: *mut u8 = std::ptr::null_mut();
            let mut num_frames: u32 = 0;
            let mut flags: u32 = 0;
            let get_ok = unsafe {
                capture_client.GetBuffer(
                    &mut data_ptr,
                    &mut num_frames,
                    &mut flags,
                    None,
                    None,
                )
            };
            match get_ok {
                Err(e) => {
                    log::warn!("IAudioCaptureClient::GetBuffer: {e}");
                    break;
                }
                Ok(()) => {}
            }

            let bytes = unsafe {
                std::slice::from_raw_parts(
                    data_ptr,
                    (num_frames as usize) * block_align,
                )
            };
            // VecDeque has `extend` but not `extend_from_slice`; use extend.
            pump.byte_queue.extend(bytes.iter().copied());

            unsafe {
                let _ = capture_client.ReleaseBuffer(num_frames);
            }
        }

        pump.drain_frames();
        pump.tick(app, &vad_tx);
    }

    unsafe {
        let _ = audio_client.Stop();
        windows::Win32::Foundation::CloseHandle(h_event).ok();
    }
    log::info!("Process loopback stream stopped (pid {pid})");
    Ok(())
}

// ── Internals ────────────────────────────────────────────────────────────────

/// Activate an `IAudioClient` for process loopback via the async COM API.
/// Blocks until the activation completes (typically < 50 ms).
fn activate_for_process(pid: u32) -> Result<IAudioClient, String> {
    let (tx, rx) = mpsc::sync_channel::<windows::core::Result<IAudioClient>>(1);
    let handler: IActivateAudioInterfaceCompletionHandler =
        ActivationHandler { tx }.into();

    // Build the activation params on the stack.
    let params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };

    // Build a raw PROPVARIANT (VT_BLOB) pointing to the params above.
    //
    // We use `windows_core::imp::PROPVARIANT` (the raw union, no Drop impl)
    // and cast it to `*const windows::core::PROPVARIANT` (repr(transparent)
    // wrapper) for the API call.  This avoids PropVariantClear attempting to
    // free a stack pointer.
    let mut raw_pv: windows_core::imp::PROPVARIANT = unsafe { std::mem::zeroed() };
    unsafe {
        let inner = &mut raw_pv.Anonymous.Anonymous;
        inner.vt = 65u16; // VT_BLOB (VARENUM is a type alias for u16 in imp)
        inner.Anonymous.blob.cbSize =
            std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32;
        inner.Anonymous.blob.pBlobData =
            &params as *const AUDIOCLIENT_ACTIVATION_PARAMS as *mut u8;
    }
    // `windows::core::PROPVARIANT` is repr(transparent) over imp::PROPVARIANT.
    let pv_ptr: *const windows::core::PROPVARIANT =
        &raw_pv as *const windows_core::imp::PROPVARIANT
            as *const windows::core::PROPVARIANT;

    unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(pv_ptr),
            &handler,
        )
        .map_err(|e| format!("ActivateAudioInterfaceAsync: {e}"))?;
    }

    // `params` is still alive here — the async operation read it before
    // ActivateAudioInterfaceAsync returned (synchronous portion of the call).
    rx.recv_timeout(Duration::from_secs(10))
        .map_err(|_| format!("process loopback activation timed out for pid {pid}"))?
        .map_err(|e| {
            // E_NOTIMPL (0x80004001): Windows has no active render stream for
            // this PID right now — the app must be playing audio when Start is clicked.
            if e.code() == windows::core::HRESULT(0x80004001_u32 as i32) {
                format!("process loopback: 目標應用程式目前沒有音訊輸出，請先讓它播放聲音再按 Start")
            } else {
                format!("process loopback activation failed: {e}")
            }
        })
}

