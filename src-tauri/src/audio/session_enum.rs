//! Enumerate Windows audio sessions to list processes currently outputting audio.
//!
//! Uses IAudioSessionManager2 on the default render endpoint.
//! Requires COM to be initialised on the calling thread.

use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioSessionControl2, IAudioSessionManager2,
    IMMDeviceEnumerator, MMDeviceEnumerator,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
// Interface trait must be in scope for .cast() to work.
use windows::core::{Interface, PWSTR};

use crate::types::AudioProcess;

/// Return all processes that currently have an active audio session on the
/// default render (speaker) device.
///
/// Duplicate PIDs (multiple sessions per process) are collapsed.
/// The "System Sounds" session (PID 0) is skipped.
pub fn list_audio_processes() -> Result<Vec<AudioProcess>, String> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| e.to_string())?;

        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| e.to_string())?;

        let session_mgr: IAudioSessionManager2 = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| e.to_string())?;

        let session_enum = session_mgr
            .GetSessionEnumerator()
            .map_err(|e| e.to_string())?;

        let count = session_enum.GetCount().map_err(|e| e.to_string())?;

        let mut results: Vec<AudioProcess> = Vec::new();
        let mut seen_pids = std::collections::HashSet::<u32>::new();

        for i in 0..count {
            let ctrl = match session_enum.GetSession(i) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Cast to the extended interface for PID access.
            let ctrl2: IAudioSessionControl2 = match ctrl.cast() {
                Ok(c) => c,
                Err(_) => continue,
            };

            // PID 0 = System Sounds — skip it.
            // NOTE: IsSystemSoundsSession() is NOT used here because in windows-rs
            // both S_OK and S_FALSE resolve to Ok(()), making is_ok() useless for
            // distinguishing system-sounds vs regular sessions. PID 0 is the
            // reliable discriminator.
            let pid = match ctrl2.GetProcessId() {
                Ok(p) if p != 0 => p,
                _ => continue,
            };

            if seen_pids.contains(&pid) {
                continue;
            }
            seen_pids.insert(pid);

            let name = process_name(pid).unwrap_or_else(|| format!("PID {pid}"));
            results.push(AudioProcess { pid, name });
        }

        // Sort by name for a consistent order.
        results.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(results)
    }
}

/// Get the basename of the executable for `pid` (e.g. `"chrome.exe"`).
/// Returns `None` on any error (process exited, insufficient permissions, etc.).
unsafe fn process_name(pid: u32) -> Option<String> {
    let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
    let mut buf = vec![0u16; 1024];
    let mut size = buf.len() as u32;
    QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size)
        .ok()?;
    windows::Win32::Foundation::CloseHandle(handle).ok();
    let full_path = String::from_utf16_lossy(&buf[..size as usize]);
    std::path::Path::new(&full_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}
