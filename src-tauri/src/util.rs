//! Small shared helpers.

use std::path::{Path, PathBuf};

/// Load `KEY=VALUE` pairs from a `.env` file into the process environment.
///
/// Exists so secrets (`OPENROUTER_API_KEY`) can live in a gitignored file next
/// to the project instead of the machine-wide environment or `settings.json`.
/// Hand-rolled rather than pulling in a crate — the format we need is a dozen
/// lines of parsing.
///
/// A variable already present in the real environment is never overwritten, so
/// `OPENROUTER_API_KEY=... cargo tauri dev` still wins over the file.
///
/// Returns the file that was loaded, if any.
pub fn load_dotenv() -> Option<PathBuf> {
    for path in dotenv_candidates() {
        if let Ok(contents) = std::fs::read_to_string(&path) {
            apply_dotenv(&contents);
            return Some(path);
        }
    }
    None
}

/// Search order: explicit override → cwd → repo root (dev) → app data → exe dir.
///
/// The app data entry is the only one that is stable. Every other candidate is
/// a function of where the binary happens to be launched from: a dev build run
/// by `cargo tauri dev` finds the repo's `.env`, and an installed build looks
/// beside itself in Program Files and finds nothing — so the same machine that
/// has been translating all day has no providers at all after installing, and
/// the whole list, not just the keys, comes from that file.
///
/// `%APPDATA%\com.bilingualsubtitle.app\.env` is next to `settings.json`,
/// writable without elevation, and survives both reinstall and uninstall.
fn dotenv_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(explicit) = std::env::var("BILINGSUBS_ENV_FILE") {
        if !explicit.is_empty() {
            out.push(PathBuf::from(explicit));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        out.push(cwd.join(".env"));
        // `cargo tauri dev` runs from src-tauri/; the file lives one level up.
        if let Some(parent) = cwd.parent() {
            out.push(parent.join(".env"));
        }
    }
    if let Some(dir) = app_data_dir() {
        out.push(dir.join(".env"));
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        out.push(dir.join(".env"));
    }
    out
}

/// `%APPDATA%\com.bilingualsubtitle.app`, the directory `settings.json`
/// already lives in.
///
/// Resolved from the environment rather than through Tauri's path API because
/// this runs before the app handle exists — the environment has to be loaded
/// before anything reads a provider out of it.
pub fn app_data_dir() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)?;
    Some(base.join("com.bilingualsubtitle.app"))
}

fn apply_dotenv(contents: &str) {
    for (key, value) in parse_dotenv(contents) {
        if std::env::var_os(&key).is_none() {
            // Real environment wins; the file only fills gaps.
            std::env::set_var(&key, &value);
        }
    }
}

/// `KEY=` means "not set", not "set to empty".
///
/// `.env.example` lists every variable with a blank value as documentation, so
/// a copied template would otherwise define them all as empty strings — and
/// `std::env::var` returns `Ok("")` for those, which silently defeats every
/// `unwrap_or_else(default)` downstream. That is exactly how `ASR_PORT=` in a
/// copied template made the app launch its sidecar on an empty port.
fn is_unset(value: &str) -> bool {
    value.is_empty()
}

/// Pure parser, split out from `apply_dotenv` so it can be tested without
/// mutating the process environment.
fn parse_dotenv(contents: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    // PowerShell's `Set-Content -Encoding utf8` writes a BOM on some versions,
    // and U+FEFF is not whitespace, so it would otherwise glue itself to the
    // first key and silently break that one variable.
    let contents = contents.strip_prefix('\u{feff}').unwrap_or(contents);
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let value = value.trim();
        // Strip one layer of matching quotes, if present.
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(value);
        if is_unset(value) {
            continue;
        }
        out.push((key.to_string(), value.to_string()));
    }
    out
}

/// Poll `url` with GET until it returns 200 or `timeout_secs` expires.
/// Used to wait for the asr-srv sidecar to come up.
pub fn wait_for_http_ok(url: &str, timeout_secs: u64) -> bool {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    while std::time::Instant::now() < deadline {
        if ureq::get(url).call().is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::parse_dotenv;

    #[test]
    fn parse_dotenv_handles_comments_quotes_and_export() {
        let pairs = parse_dotenv(
            "# a comment\n\
             \n\
             OPENROUTER_API_KEY=sk-or-v1-abc\n\
             export OPENROUTER_MODEL=\"google/gemini-2.5-flash-lite\"\n\
             QUOTED='single'\n\
               SPACED  =  value  \n\
             not_a_pair\n\
             =novalue\n",
        );
        assert_eq!(
            pairs,
            vec![
                ("OPENROUTER_API_KEY".into(), "sk-or-v1-abc".into()),
                ("OPENROUTER_MODEL".into(), "google/gemini-2.5-flash-lite".into()),
                ("QUOTED".into(), "single".into()),
                ("SPACED".into(), "value".into()),
            ]
        );
    }

    #[test]
    fn parse_dotenv_treats_a_blank_value_as_unset() {
        // .env.example ships every variable blank as documentation. A copied
        // template must not define them as empty strings: std::env::var would
        // then return Ok("") and defeat every unwrap_or_else(default).
        let pairs = parse_dotenv("ASR_PORT=\nWHISPER_MODEL=   \nQUOTED=\"\"\nREAL=9001\n");
        assert_eq!(pairs, vec![("REAL".to_string(), "9001".to_string())]);
    }

    #[test]
    fn parse_dotenv_ignores_a_leading_bom() {
        let pairs = parse_dotenv("\u{feff}TRANSLATE_AISTUDIO_API_KEY=abc\n");
        assert_eq!(pairs, vec![("TRANSLATE_AISTUDIO_API_KEY".into(), "abc".into())]);
    }

    #[test]
    fn parse_dotenv_keeps_inner_equals_and_urls() {
        let pairs = parse_dotenv("OPENROUTER_BASE_URL=https://openrouter.ai/api/v1\nA=b=c\n");
        assert_eq!(
            pairs,
            vec![
                ("OPENROUTER_BASE_URL".into(), "https://openrouter.ai/api/v1".into()),
                ("A".into(), "b=c".into()),
            ]
        );
    }
}
