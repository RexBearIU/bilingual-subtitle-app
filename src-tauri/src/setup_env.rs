//! First-run creation of the ASR sidecar's Python environment.
//!
//! The sidecar is Python (ADR-0001 sidecar-first, ADR-0006 faster-whisper), so
//! the app needs an interpreter with ~1.2 GB of wheels in it before it can
//! transcribe anything. A dev build borrows the repo's `.venv`; an installed
//! build has no repo, and until this module existed the answer was "open a
//! terminal and run `uv sync`" — which is not an answer you can give someone
//! you handed an installer to.
//!
//! What happens instead: `uv.exe`, `pyproject.toml` and `uv.lock` ship as
//! bundled resources, the manifests are copied into the app data directory,
//! and `uv sync` builds the venv there — the one location `resolve_python`
//! looks in that does not depend on where the binary was launched from.
//!
//! uv is bundled rather than required (+46 MB to the installer) for the same
//! reason: "first install uv" is the same terminal step wearing a hat.
//!
//! Progress is streamed as `setup_progress` events rather than collected at the
//! end. The download is ~1.2 GB and several minutes; a window that says nothing
//! for that long reads as a hang, and people kill it.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

#[cfg(target_os = "windows")]
const NO_WINDOW: u32 = 0x0800_0000; // CREATE_NO_WINDOW

/// One line of setup progress, or the terminal result.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupProgress {
    /// A line from uv, forwarded as-is. Empty on the terminal event.
    pub line: String,
    /// True only on the last event of a run.
    pub done: bool,
    /// Meaningful only when `done`.
    pub ok: bool,
    /// Human-readable outcome, set when `done`.
    pub message: String,
}

/// Where the environment is built: next to `settings.json` and the `.env`.
pub fn env_root() -> Option<PathBuf> {
    crate::util::app_data_dir()
}

/// Whether an interpreter with the sidecar's dependencies already exists.
///
/// Deliberately the same question `resolve_python` answers, asked before
/// launching anything: a `PYTHON_BIN` pointing at someone's own venv counts,
/// and so does a repo checkout in a dev build. Setup is for the case where
/// neither is true, and offering it to someone who already has a working
/// environment would just be a 1.2 GB no-op.
pub fn is_ready() -> bool {
    if std::env::var("PYTHON_BIN").is_ok_and(|v| !v.is_empty()) {
        return true;
    }
    crate::commands::find_venv_python().is_some()
}

/// Build the environment. Blocking; call it on a worker thread.
///
/// Emits `setup_progress` throughout and once more with `done: true`.
pub fn run(app: &AppHandle) {
    match build(app) {
        Ok(python) => {
            log::info!("setup: environment ready at {python}");
            emit(app, SetupProgress {
                line: String::new(),
                done: true,
                ok: true,
                message: format!("Ready — {python}"),
            });
        }
        Err(e) => {
            log::error!("setup failed: {e}");
            emit(app, SetupProgress {
                line: String::new(),
                done: true,
                ok: false,
                message: e,
            });
        }
    }
}

fn build(app: &AppHandle) -> Result<String, String> {
    let root = env_root().ok_or("no app data directory to build the environment in")?;
    std::fs::create_dir_all(&root).map_err(|e| format!("create {}: {e}", root.display()))?;

    // uv resolves against the manifests in the project directory, so they have
    // to be beside the venv rather than read from the bundle in place: the
    // bundle lives in Program Files, which is not writable without elevation.
    for name in ["pyproject.toml", "uv.lock"] {
        let src = crate::commands::resolve_resource_path(name)
            .ok_or_else(|| format!("bundled {name} is missing from this build"))?;
        let dst = root.join(name);
        std::fs::copy(&src, &dst)
            .map_err(|e| format!("copy {name} to {}: {e}", dst.display()))?;
        step(app, format!("staged {name}"));
    }

    let uv = crate::commands::resolve_resource_path("uv.exe")
        .ok_or("bundled uv.exe is missing from this build")?;

    step(app, "resolving dependencies (this downloads ~1.2 GB on a first run)".into());

    let mut cmd = Command::new(&uv);
    cmd.arg("sync")
        .arg("--project")
        .arg(&root)
        // Install exactly what the lockfile says. Without this uv is free to
        // re-resolve, so the environment someone gets from the installer could
        // differ from the one every measurement in bench/ was taken against —
        // and it would rewrite the uv.lock we just staged.
        .arg("--frozen")
        // uv would otherwise render progress bars with carriage returns, which
        // arrive as one enormous line with no newline in it.
        .arg("--no-progress")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(NO_WINDOW);
    }

    let mut child = cmd.spawn().map_err(|e| format!("launch uv: {e}"))?;

    // uv writes its progress to stderr and its results to stdout; both are
    // worth showing, and neither alone tells you what is happening.
    if let Some(err) = child.stderr.take() {
        let app = app.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                step(&app, line);
            }
        });
    }
    if let Some(out) = child.stdout.take() {
        let app = app.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                step(&app, line);
            }
        });
    }

    let status = child.wait().map_err(|e| format!("wait for uv: {e}"))?;
    if !status.success() {
        return Err(format!(
            "uv sync exited with {}. The lines above say why; a failed download \
             is usually the network and is safe to retry.",
            status.code().map_or_else(|| "a signal".into(), |c| c.to_string()),
        ));
    }

    crate::commands::find_venv_python().ok_or_else(|| {
        format!(
            "uv reported success but no interpreter appeared under {}",
            root.display()
        )
    })
}

fn step(app: &AppHandle, line: String) {
    if line.trim().is_empty() {
        return;
    }
    log::info!("setup: {line}");
    emit(app, SetupProgress { line, done: false, ok: false, message: String::new() });
}

fn emit(app: &AppHandle, p: SetupProgress) {
    let _ = app.emit("setup_progress", p);
}
