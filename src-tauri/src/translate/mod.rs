//! Translation pipeline: receives `TranslationRequest`s from the ASR worker
//! and calls a hosted OpenAI-compatible chat-completions endpoint to produce
//! subtitles in the selected target language.
//!
//! Providers are configuration, not code. Any endpoint speaking the
//! OpenAI-compatible `/chat/completions` shape works — OpenRouter, Google AI
//! Studio, Groq — and several are listed in preference order so one failing
//! does not take translation down with it.
//!
//! The ordered list lives in `settings.json` and is owned by the Settings
//! panel: add, remove, reorder, edit. `.env` is still honoured, but only as a
//! place to keep API keys out of the settings file — see `resolve_key`.

pub mod remote;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::settings::{PersistSettings, SavedProvider, SettingsPath};
use crate::types::SubtitleMode;

/// Built-in presets, so a well-known provider needs only a name and a key.
///
/// `label` is what the UI shows. It is separate from `name` because `name` is
/// an identity: it keys the stored API key and `TRANSLATE_<NAME>_API_KEY`, and
/// it is what the logs say. Tying the display text to that would mean a
/// cosmetic rename silently orphans a key.
///
/// `(name, label, base_url, default_model)`
const PRESETS: &[(&str, &str, &str, &str)] = &[
    (
        "aistudio",
        "Google AI Studio",
        "https://generativelanguage.googleapis.com/v1beta/openai/",
        "gemini-3.5-flash-lite",
    ),
    (
        "openrouter",
        "OpenRouter",
        "https://openrouter.ai/api/v1",
        "google/gemini-3.5-flash-lite",
    ),
    (
        // Measured on this account: 500 ms median, no empty responses — faster
        // than either Gemini route. The alternatives are worse for subtitles:
        // qwen3.6 emits a <think> preamble every call, and both gpt-oss sizes
        // returned empty content on 1 line in 4.
        "groq",
        "Groq",
        "https://api.groq.com/openai/v1",
        "qwen/qwen3.8-27b",
    ),
];

/// Used when nothing is configured at all.
pub const DEFAULT_PROVIDER: &str = "openrouter";

/// A built-in preset as the Settings panel's add form sees it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetInfo {
    pub name: String,
    pub label: String,
}

/// Presets offered by the add form: pick one and only a key is needed.
pub fn preset_list() -> Vec<PresetInfo> {
    PRESETS
        .iter()
        .map(|(n, label, _, _)| PresetInfo { name: (*n).into(), label: (*label).into() })
        .collect()
}

fn preset(name: &str) -> Option<(&'static str, &'static str)> {
    PRESETS
        .iter()
        .find(|(n, _, _, _)| *n == name)
        .map(|(_, _, base, model)| (*base, *model))
}

/// The display text for a name with no label of its own.
///
/// Falls back to the name itself, so a hand-rolled entry still reads as
/// something rather than as a blank row.
fn default_label(name: &str) -> String {
    PRESETS
        .iter()
        .find(|(n, _, _, _)| *n == name)
        .map(|(_, label, _, _)| (*label).to_string())
        .unwrap_or_else(|| name.to_string())
}

// ── the live provider list ──────────────────────────────────────────────────

/// One configured endpoint, ready to call.
///
/// Deliberately NOT stored in `AppState`: that derives `Debug` and is logged on
/// state changes, which would print the API key.
pub struct Provider {
    /// Config key (`aistudio`, `openrouter`, …).
    pub name: String,
    /// What the UI and the logs show. Never used to look anything up.
    pub label: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// Optional upstream preference — OpenRouter's `provider.order`.
    pub provider_order: Option<Vec<String>>,
    /// Where the key came from, for the UI. Never the key itself.
    pub key_source: KeySource,
    /// Whether this entry can actually be called. A non-`Ready` provider is
    /// kept so the UI can show it, and skipped by `pick_provider`.
    pub readiness: Readiness,
    /// Learned at runtime: this endpoint rejects `reasoning.enabled=false`.
    ///
    /// Google's OpenAI-compatible endpoint rejects it on EVERY call, so
    /// remembering the first rejection saves a wasted round trip per subtitle.
    /// Per-provider because it is a property of the endpoint, not the app.
    pub reasoning_unsupported: AtomicBool,
}

/// Why an entry cannot be called, or `Ready` if it can.
///
/// An unusable entry stays in the list rather than being dropped. Dropping it
/// meant clearing a key made the row vanish from Settings while still sitting
/// in `settings.json` — invisible, and so impossible to fix or delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Readiness {
    Ready,
    MissingKey,
    MissingUrl,
    MissingModel,
}

impl Readiness {
    /// Reason for the log. Empty when ready.
    ///
    /// English, like every other log line here. The UI writes its own Chinese
    /// wording from the enum rather than displaying this.
    pub fn reason(self) -> &'static str {
        match self {
            Readiness::Ready => "",
            Readiness::MissingKey => "no API key",
            Readiness::MissingUrl => "no base URL",
            Readiness::MissingModel => "no model",
        }
    }
}

/// Where a provider's API key was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeySource {
    /// Typed into the Settings panel; stored in `settings.json`.
    Settings,
    /// `TRANSLATE_<NAME>_API_KEY` (or legacy `OPENROUTER_API_KEY`).
    Env,
}

impl Provider {
    /// `{base_url}/chat/completions`, tolerating a trailing slash.
    pub fn completions_url(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }

    pub fn key_url(&self) -> String {
        format!("{}/models", self.base_url.trim_end_matches('/'))
    }

    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            name: self.name.clone(),
            label: self.label.clone(),
            model: self.model.clone(),
            base_url: self.base_url.clone(),
            key_source: self.key_source,
            readiness: self.readiness,
        }
    }
}

/// A provider as the UI sees it — identity only.
///
/// Separate from `Provider` because this one crosses the IPC boundary and is
/// stored in `AppState`, which derives `Debug` and is logged. Carrying the key
/// in a type used that way is how keys end up in log files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub name: String,
    /// Display text; falls back to the preset's label, then to `name`.
    pub label: String,
    pub model: String,
    pub base_url: String,
    pub key_source: KeySource,
    pub readiness: Readiness,
}

/// The live, ordered provider list, shared between the Settings commands and
/// the translate worker.
///
/// Managed Tauri state rather than a field on `AppState` for the same reason
/// `Provider` is: the keys must stay out of anything that derives `Debug`.
/// `Arc<Provider>` entries so the worker can take one out from under the lock
/// and hold it across an HTTP call without blocking an edit.
#[derive(Default)]
pub struct Registry(pub Mutex<Vec<Arc<Provider>>>);

/// The provider the worker should use, given the shared active index.
///
/// Takes the index modulo the length, because the list can be edited from the
/// UI between one subtitle and the next and may have shrunk under a stale
/// index. Returns `None` when nothing in the list is callable.
///
/// Entries that are not `Ready` are skipped rather than dropped from the list,
/// so their row stays visible in Settings. The found index is written back to
/// `active`, so the "in use" badge names the provider actually being called
/// and not the unusable one that was selected.
pub fn pick_provider(app: &AppHandle, active: &AtomicUsize) -> Option<(usize, Arc<Provider>)> {
    let reg = app.try_state::<Registry>()?;
    let list = reg.0.lock().ok()?;
    if list.is_empty() {
        return None;
    }
    let start = active.load(Ordering::Relaxed) % list.len();
    for step in 0..list.len() {
        let i = (start + step) % list.len();
        if list[i].readiness == Readiness::Ready {
            if i != start {
                active.store(i, Ordering::Relaxed);
            }
            return Some((i, Arc::clone(&list[i])));
        }
    }
    None
}

/// Rebuild the live list from `settings.json` + the environment, then publish
/// the key-free view to the UI.
///
/// Called at startup and after every edit, so the list the panel shows and the
/// list the worker calls can never drift apart — which matters because the UI
/// addresses providers by index.
pub fn refresh(app: &AppHandle) -> Vec<ProviderInfo> {
    let providers = build_all(app);
    let infos: Vec<ProviderInfo> = providers.iter().map(|p| p.info()).collect();

    if let Some(reg) = app.try_state::<Registry>() {
        if let Ok(mut list) = reg.0.lock() {
            *list = providers;
        }
    }
    crate::state::update_and_emit(app, |s| s.translate_providers = infos.clone());
    infos
}

/// Human-readable list for the log. Names and models only.
pub fn describe(infos: &[ProviderInfo]) -> String {
    if infos.is_empty() {
        return "(none configured)".into();
    }
    infos
        .iter()
        .map(|p| match p.readiness {
            Readiness::Ready => format!("{}({})", p.label, p.model),
            r => format!("{}[{}]", p.label, r.reason()),
        })
        .collect::<Vec<_>>()
        .join(" → ")
}

// ── resolution ──────────────────────────────────────────────────────────────

fn load_settings(app: &AppHandle) -> PersistSettings {
    // `Mutex<SettingsPath>`, not `SettingsPath`: that is how lib.rs manages it.
    // Asking for the bare type always missed, which silently made the key
    // stored by the Settings panel unreadable.
    app.try_state::<Mutex<SettingsPath>>()
        .and_then(|p| p.lock().ok().map(|p| PersistSettings::load(&p.0)))
        .unwrap_or_default()
}

/// Build the full ordered list from settings, seeding it from the environment
/// the first time.
fn build_all(app: &AppHandle) -> Vec<Arc<Provider>> {
    let mut saved = load_settings(app).providers;

    // First run with a `.env`-configured setup: adopt that list so the panel
    // has something to show and reorder. Names, URLs and models only — keys
    // stay in `.env` and are resolved per call, so nothing secret is copied
    // into a second file.
    if saved.is_empty() {
        saved = seed_from_env();
        if saved.is_empty() {
            saved.extend(seed_from_legacy(app));
        }
        if !saved.is_empty() {
            log::info!(
                "TL: seeding provider list from existing config: {}",
                saved.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "),
            );
            persist(app, &saved);
        }
    }

    let out: Vec<Arc<Provider>> = saved.iter().map(|s| Arc::new(build_one(s))).collect();

    let broken: Vec<String> = out
        .iter()
        .filter(|p| p.readiness != Readiness::Ready)
        .map(|p| format!("{} ({})", p.name, p.readiness.reason()))
        .collect();
    if !broken.is_empty() {
        log::warn!("TL: not callable, shown in Settings but skipped: {}", broken.join(", "));
    }
    out
}

fn seed_from_env() -> Vec<SavedProvider> {
    let Some(list) = non_empty_env("TRANSLATE_PROVIDERS") else {
        return Vec::new();
    };
    list.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|name| {
            let upper = env_prefix(name);
            SavedProvider {
                name: name.to_string(),
                // Left empty so the preset's label applies, and keeps applying
                // if it is ever improved.
                label: String::new(),
                // Left empty when a preset covers it, so the preset stays
                // authoritative if it is ever corrected.
                base_url: non_empty_env(&format!("TRANSLATE_{upper}_BASE_URL")).unwrap_or_default(),
                api_key: String::new(),
                model: non_empty_env(&format!("TRANSLATE_{upper}_MODEL")).unwrap_or_default(),
            }
        })
        .collect()
}

fn env_prefix(name: &str) -> String {
    name.to_uppercase().replace('-', "_")
}

/// Resolve one saved entry into a callable provider, or `None` if it has no key.
/// Resolve one saved entry, filling blanks from the environment and the
/// preset. Always returns a `Provider`: one that could not be completed comes
/// back with a non-`Ready` `readiness` and empty fields, so the UI can show the
/// row and say what is missing instead of the entry disappearing.
fn build_one(s: &SavedProvider) -> Provider {
    let upper = env_prefix(&s.name);
    let (preset_base, preset_model) = preset(&s.name).unwrap_or(("", ""));

    let key = resolve_key(s, &upper);

    let base_url = non_empty(s.base_url.clone())
        .or_else(|| non_empty_env(&format!("TRANSLATE_{upper}_BASE_URL")))
        .or_else(|| non_empty(preset_base.to_string()));

    let model = non_empty(s.model.clone())
        .or_else(|| non_empty_env(&format!("TRANSLATE_{upper}_MODEL")))
        .or_else(|| non_empty(preset_model.to_string()));

    // URL and model first: they are what makes an entry structurally wrong,
    // and reporting "no key" for a row that also has nowhere to send it would
    // send the user off to fetch a key they cannot yet use.
    let readiness = if base_url.is_none() {
        Readiness::MissingUrl
    } else if model.is_none() {
        Readiness::MissingModel
    } else if key.is_none() {
        Readiness::MissingKey
    } else {
        Readiness::Ready
    };

    let (api_key, key_source) = key.unwrap_or((String::new(), KeySource::Settings));

    Provider {
        name: s.name.clone(),
        label: non_empty(s.label.clone()).unwrap_or_else(|| default_label(&s.name)),
        base_url: base_url.unwrap_or_default(),
        api_key,
        model: model.unwrap_or_default(),
        provider_order: non_empty_env(&format!("TRANSLATE_{upper}_PROVIDER_ORDER")).map(split_csv),
        key_source,
        readiness,
        reasoning_unsupported: AtomicBool::new(false),
    }
}

/// The key for a saved entry: what was typed into Settings, else the
/// environment.
///
/// Settings first, so changing it in the panel visibly wins. The environment
/// fallback is what lets a `.env` keep holding the secrets while the ordered
/// list lives in `settings.json`.
fn resolve_key(s: &SavedProvider, upper: &str) -> Option<(String, KeySource)> {
    if let Some(k) = non_empty(s.api_key.clone()) {
        return Some((k, KeySource::Settings));
    }
    if let Some(k) = non_empty_env(&format!("TRANSLATE_{upper}_API_KEY")) {
        return Some((k, KeySource::Env));
    }
    // The original variable, for a setup that predates the per-provider names.
    if s.name == DEFAULT_PROVIDER {
        if let Some(k) = non_empty_env("OPENROUTER_API_KEY") {
            return Some((k, KeySource::Env));
        }
    }
    None
}

/// One-shot migration of the original single-provider config into the list.
///
/// Runs only when the list is empty, and moves the stored key rather than
/// copying it, so a key ends up in exactly one place. Migrating instead of
/// keeping a permanent fallback matters for delete: a fallback that reappears
/// after the user removes the last entry looks like the button is broken.
fn seed_from_legacy(app: &AppHandle) -> Option<SavedProvider> {
    let cfg = load_settings(app);
    let stored = non_empty(cfg.openrouter_api_key.clone());
    // An env-only setup has nothing to migrate but still needs an entry, so the
    // key can be resolved from OPENROUTER_API_KEY at call time.
    if stored.is_none() && non_empty_env("OPENROUTER_API_KEY").is_none() {
        return None;
    }

    let base_url = non_empty_env("OPENROUTER_BASE_URL").unwrap_or_default();

    // Name it after the endpoint actually in use, so the logs and the UI do not
    // say "openrouter" while pointing at Google.
    let name = PRESETS
        .iter()
        .find(|(_, _, base, _)| {
            !base_url.is_empty() && base_url.trim_end_matches('/') == base.trim_end_matches('/')
        })
        .map(|(n, _, _, _)| (*n).to_string())
        .unwrap_or_else(|| DEFAULT_PROVIDER.to_string());

    Some(SavedProvider {
        name,
        label: String::new(),
        base_url: if preset_covers(&base_url) { String::new() } else { base_url },
        api_key: stored.unwrap_or_default(),
        model: non_empty_env("OPENROUTER_MODEL")
            .or_else(|| non_empty(cfg.openrouter_model.clone()))
            .unwrap_or_default(),
    })
}

/// True when a preset already supplies this base URL, so it need not be stored.
fn preset_covers(base_url: &str) -> bool {
    PRESETS
        .iter()
        .any(|(_, _, base, _)| base_url.trim_end_matches('/') == base.trim_end_matches('/'))
}

// ── editing ─────────────────────────────────────────────────────────────────

/// One entry as the Settings panel sends it back.
///
/// `api_key` is three-valued on purpose, because the panel never receives the
/// stored key and so cannot echo it: absent means "leave it alone", empty
/// means "clear it, fall back to the environment", and a value replaces it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDraft {
    pub name: String,
    /// Display text. Empty = the preset's label, else the name.
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub api_key: Option<String>,
}

/// Replace the whole list — this is add, remove, edit and reorder at once.
///
/// One command rather than four because the panel owns the order, so every
/// edit is "here is the new list" anyway, and a single write cannot leave the
/// stored order disagreeing with what the user just dragged.
pub fn set_list(app: &AppHandle, drafts: Vec<ProviderDraft>) -> Result<Vec<ProviderInfo>, String> {
    let existing = load_settings(app).providers;

    let mut out: Vec<SavedProvider> = Vec::with_capacity(drafts.len());
    for d in drafts {
        let name = d.name.trim().to_string();
        if name.is_empty() {
            return Err("provider name must not be empty".into());
        }
        if out.iter().any(|p| p.name == name) {
            return Err(format!("duplicate provider name: {name}"));
        }
        // Carry the stored key forward when the panel did not send one.
        let api_key = match d.api_key {
            Some(k) => k.trim().to_string(),
            None => existing
                .iter()
                .find(|p| p.name == name)
                .map(|p| p.api_key.clone())
                .unwrap_or_default(),
        };
        out.push(SavedProvider {
            name,
            label: d.label.trim().to_string(),
            base_url: d.base_url.trim().to_string(),
            api_key,
            model: d.model.trim().to_string(),
        });
    }

    persist(app, &out);
    // Never the keys, and never the URLs either — a base URL can carry a token.
    log::info!(
        "TL: provider list set to [{}]",
        out.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "),
    );
    Ok(refresh(app))
}

fn persist(app: &AppHandle, providers: &[SavedProvider]) {
    let Some(sp) = app.try_state::<Mutex<SettingsPath>>() else { return };
    let Ok(sp) = sp.lock() else { return };
    let mut cfg = PersistSettings::load(&sp.0);
    cfg.providers = providers.to_vec();
    // The list is now the only home for keys; leaving a copy in the legacy
    // field would be a second plaintext secret nobody reads.
    cfg.openrouter_api_key = String::new();
    if let Err(e) = cfg.save(&sp.0) {
        log::warn!("TL: saving provider list failed: {e}");
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn split_csv(v: String) -> Vec<String> {
    v.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn non_empty(s: String) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(non_empty)
}

/// Sent from the ASR worker to the translation worker for each transcribed chunk.
pub struct TranslationRequest {
    /// Stable segment identifier — same as the source `subtitle_update` id.
    pub id: String,
    /// ISO-639-1 source language (`"ko"` / `"en"` / `"zh"`).
    pub source_lang: String,
    /// Source text as returned by asr-srv.
    pub source_text: String,
    /// Active subtitle display mode (drives which target language we need).
    pub mode: SubtitleMode,
    pub started_at_ms: u64,
    pub ended_at_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(name: &str, base: &str, key: &str, model: &str) -> SavedProvider {
        SavedProvider {
            name: name.into(),
            label: String::new(),
            base_url: base.into(),
            api_key: key.into(),
            model: model.into(),
        }
    }

    #[test]
    fn a_preset_name_gets_the_presets_display_label() {
        let p = build_one(&saved("groq", "", "k", ""));
        assert_eq!(p.label, "Groq");
        assert_eq!(p.name, "groq", "the identity is untouched by the label");
    }

    #[test]
    fn an_unknown_name_is_its_own_label() {
        let p = build_one(&saved("mine", "https://x.test/v1", "k", "m"));
        assert_eq!(p.label, "mine");
    }

    #[test]
    fn a_typed_label_wins_over_the_preset() {
        let mut s = saved("groq", "", "k", "");
        s.label = "  快的那個  ".into();
        // Trimmed on the way in by `set_list`; untrimmed input still resolves.
        let p = build_one(&s);
        assert_eq!(p.label.trim(), "快的那個");
    }

    #[test]
    fn presets_cover_the_documented_names() {
        for name in ["aistudio", "openrouter", "groq"] {
            let (base, model) = preset(name).expect("preset missing");
            assert!(base.starts_with("https://"), "{name} base_url");
            assert!(!model.is_empty(), "{name} model");
        }
        assert!(preset("nope").is_none());
    }

    #[test]
    fn completions_url_tolerates_a_trailing_slash() {
        // Google's documented base URL ends in '/', OpenRouter's does not.
        let mk = |base: &str| Provider {
            name: "t".into(),
            label: "t".into(),
            base_url: base.into(),
            api_key: "k".into(),
            model: "m".into(),
            provider_order: None,
            key_source: KeySource::Settings,
            readiness: Readiness::Ready,
            reasoning_unsupported: AtomicBool::new(false),
        };
        assert_eq!(
            mk("https://x/v1/").completions_url(),
            "https://x/v1/chat/completions"
        );
        assert_eq!(
            mk("https://x/v1").completions_url(),
            "https://x/v1/chat/completions"
        );
    }

    #[test]
    fn split_csv_trims_and_drops_blanks() {
        assert_eq!(split_csv(" a , ,b ".into()), vec!["a".to_string(), "b".into()]);
    }

    #[test]
    fn build_one_fills_blanks_from_the_preset() {
        // A preset name needs only a key; the URL and model come from PRESETS.
        let p = build_one(&saved("groq", "", "k", ""));
        assert_eq!(p.readiness, Readiness::Ready);
        assert_eq!(p.base_url, "https://api.groq.com/openai/v1");
        assert_eq!(p.model, "qwen/qwen3.8-27b");
        assert_eq!(p.key_source, KeySource::Settings);
    }

    #[test]
    fn build_one_prefers_what_the_user_typed() {
        let p = build_one(&saved("groq", "https://proxy.local/v1", "k", "my-model"));
        assert_eq!(p.base_url, "https://proxy.local/v1");
        assert_eq!(p.model, "my-model");
    }

    #[test]
    fn an_incomplete_entry_survives_and_says_what_is_missing() {
        // Every one of these used to be dropped from the list, which made the
        // row disappear from Settings while staying in settings.json.
        // NOTE: assumes no TRANSLATE_GROQ_API_KEY / OPENROUTER_API_KEY in the
        // test environment; `resolve_key` would otherwise find one.
        assert_eq!(build_one(&saved("groq", "", "", "")).readiness, Readiness::MissingKey);
        assert_eq!(
            build_one(&saved("mystery", "", "k", "m")).readiness,
            Readiness::MissingUrl,
        );
        assert_eq!(
            build_one(&saved("mystery", "https://x/v1", "k", "")).readiness,
            Readiness::MissingModel,
        );
    }

    #[test]
    fn a_missing_url_is_reported_before_a_missing_key() {
        // Both are missing. Naming the key would send the user to fetch one
        // they still could not use.
        assert_eq!(build_one(&saved("mystery", "", "", "")).readiness, Readiness::MissingUrl);
    }

    #[test]
    fn readiness_reasons_are_only_empty_when_ready() {
        assert!(Readiness::Ready.reason().is_empty());
        for r in [Readiness::MissingKey, Readiness::MissingUrl, Readiness::MissingModel] {
            assert!(!r.reason().is_empty(), "{r:?}");
        }
    }
}
