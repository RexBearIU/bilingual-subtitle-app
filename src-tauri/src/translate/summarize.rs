//! Derives the session's context note from what has actually been said.
//!
//! The manual note in Settings is the reliable version but nobody wants to
//! type one before every stream. This builds the same thing from the first
//! minute of subtitles: proper nouns and a sentence of topic, fed back into
//! the ASR prompt and the translation system prompt exactly like a typed note.
//!
//! It summarises *transcript*, not audio — no model here takes sound, and the
//! transcript is the better input anyway, since it is what the two consumers
//! of the context are working on.
//!
//! Runs on its own thread. The translate worker is serial, so a summary call
//! made there would sit in front of a subtitle; here it cannot delay anything.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::AppHandle;

use crate::state;
use crate::translate::{self, Provider};

/// Lines of final transcript before the first summary is worth asking for.
///
/// Too few and the model is guessing from one sentence; the cost of waiting is
/// only that early subtitles miss the benefit.
const MIN_LINES: usize = 8;

/// How often to rebuild it. Long enough not to matter as a cost, short enough
/// that switching to a different video catches up within a few subtitles.
const REFRESH: Duration = Duration::from_secs(300);

/// Transcript lines the summary is built from — the most recent ones.
const WINDOW_LINES: usize = 40;

/// Ceiling on the generated note, matching the manual field's own cap.
const MAX_CHARS: usize = 400;

/// Final source lines seen this session, oldest first.
///
/// Written by the translate worker, which already sees every final, so the ASR
/// path stays untouched.
#[derive(Default)]
pub struct Transcript(pub Mutex<Vec<String>>);

impl Transcript {
    pub fn push(&self, line: &str) {
        let Ok(mut v) = self.0.lock() else { return };
        v.push(line.to_string());
        // Only the tail is ever read; without this a long session grows without
        // bound for no benefit.
        if v.len() > WINDOW_LINES * 2 {
            let cut = v.len() - WINDOW_LINES;
            let keep = v.split_off(cut);
            *v = keep;
        }
    }

    fn recent(&self) -> Vec<String> {
        let Ok(v) = self.0.lock() else { return Vec::new() };
        let start = v.len().saturating_sub(WINDOW_LINES);
        v[start..].to_vec()
    }

    fn len(&self) -> usize {
        self.0.lock().map(|v| v.len()).unwrap_or(0)
    }
}

/// Spawn the background summariser (detached). Exits when `stop` is set.
pub fn start_summary_worker(
    app: AppHandle,
    stop: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
    transcript: Arc<Transcript>,
) {
    std::thread::Builder::new()
        .name("tl-summary".into())
        .spawn(move || summary_loop(&app, &stop, &active, &transcript))
        .expect("spawn tl-summary thread");
}

fn summary_loop(
    app: &AppHandle,
    stop: &Arc<AtomicBool>,
    active: &Arc<AtomicUsize>,
    transcript: &Arc<Transcript>,
) {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(20))
        .build();

    let mut last: Option<Instant> = None;

    loop {
        // Short sleeps so stopping the pipeline is not held up by the interval.
        std::thread::sleep(Duration::from_millis(500));
        if stop.load(Ordering::Relaxed) {
            break;
        }

        let due = match last {
            None => transcript.len() >= MIN_LINES,
            Some(t) => t.elapsed() >= REFRESH && transcript.len() >= MIN_LINES,
        };
        if !due {
            continue;
        }

        // A typed note is the user's own answer to this question, so there is
        // nothing to derive. Checked here rather than at startup because it can
        // be filled in mid-session.
        if state::read_state(app, |s| !s.context.trim().is_empty()).unwrap_or(false) {
            last = Some(Instant::now());
            continue;
        }

        let lines = transcript.recent();
        let Some((_, provider)) = translate::pick_provider(app, active) else {
            log::debug!("summary: no callable provider — skipped");
            last = Some(Instant::now());
            continue;
        };

        let t = Instant::now();
        match request_summary(&agent, &provider, &lines) {
            Ok(note) if !note.is_empty() => {
                log::info!("summary ({} ms): {note:?}", t.elapsed().as_millis());
                state::update_and_emit(app, |s| s.auto_context = note.clone());
            }
            Ok(_) => log::debug!("summary: empty response — keeping the previous note"),
            Err(e) => log::warn!("summary failed ({e}) — keeping the previous note"),
        }
        last = Some(Instant::now());
    }

    log::info!("summary worker exited");
}

/// Ask the provider what this session is about.
fn request_summary(
    agent: &ureq::Agent,
    provider: &Provider,
    lines: &[String],
) -> Result<String, String> {
    // The "say only what is supported" clause is not boilerplate. On eight lines
    // of song lyrics the first version answered "the song \"Maybe I\" by the
    // artist Maybe I" — a title and an artist invented out of one repeated
    // phrase. This note is fed back into both models, so a confident guess is
    // worse than a thin answer.
    let system = "You are given consecutive subtitle lines from a live stream or video, \
                  produced by speech recognition, so some words will be wrong. \
                  Reply with a single short paragraph, under 60 words, naming what this is \
                  (the show, game, topic) and the proper nouns that recur — people, teams, \
                  places, titles — spelled the way they should be. \
                  State only what the lines actually support. Never invent a title, an \
                  artist or a name to fill the gap, and never repeat a phrase back as if \
                  it were a proper noun. If the lines do not identify a subject, reply \
                  with exactly UNKNOWN and nothing else. \
                  It will be used as background for translating later lines. \
                  No preamble, no bullet points, no commentary on the transcription quality.";

    let body = serde_json::json!({
        "model": provider.model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user",   "content": lines.join("\n") },
        ],
        // Room for the paragraph and nothing else.
        "max_tokens": 160,
        "temperature": 0,
    });

    let json = super::remote::post_with_retry(agent, provider, &body)?;
    let raw = json
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    Ok(usable_note(&super::remote::strip_think_tags(raw)))
}

/// The note to keep, or empty when the model could not identify a subject.
///
/// "The subject is not clear from these lines" is a true answer and a useless
/// note: it would spend half of whisper's prompt budget saying nothing. Asking
/// for a sentinel makes that case detectable, rather than a phrase to
/// pattern-match in whatever language the model chose.
fn usable_note(reply: &str) -> String {
    let t = reply.trim();
    if t.trim_end_matches('.').eq_ignore_ascii_case("unknown") {
        return String::new();
    }
    t.chars().take(MAX_CHARS).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_unidentified_subject_becomes_no_note() {
        assert_eq!(usable_note("UNKNOWN"), "");
        assert_eq!(usable_note("  unknown.  "), "");
        assert_eq!(usable_note("Unknown"), "");
    }

    #[test]
    fn a_real_note_survives_intact() {
        let note = "LCK broadcast, T1 vs Gen.G. Names: Faker, Chovy, Zeus.";
        assert_eq!(usable_note(note), note);
    }

    #[test]
    fn a_note_that_merely_mentions_the_word_is_kept() {
        // Only a bare sentinel means "nothing to say".
        let note = "A quiz show; the recurring answer is unknown to the contestants.";
        assert_eq!(usable_note(note), note);
    }

    #[test]
    fn a_long_note_is_capped() {
        assert_eq!(usable_note(&"x".repeat(900)).chars().count(), MAX_CHARS);
    }

    use super::*;

    #[test]
    fn the_transcript_keeps_only_a_bounded_tail() {
        let t = Transcript::default();
        for i in 0..(WINDOW_LINES * 5) {
            t.push(&format!("line {i}"));
        }
        assert!(t.len() <= WINDOW_LINES * 2, "grew to {}", t.len());
        // Whatever it dropped, the newest line survives.
        assert_eq!(
            t.recent().last().map(String::as_str),
            Some(format!("line {}", WINDOW_LINES * 5 - 1).as_str()),
        );
    }

    #[test]
    fn recent_returns_at_most_one_window() {
        let t = Transcript::default();
        for i in 0..(WINDOW_LINES + 7) {
            t.push(&format!("line {i}"));
        }
        assert_eq!(t.recent().len(), WINDOW_LINES);
    }

    #[test]
    fn an_empty_transcript_summarises_to_nothing() {
        let t = Transcript::default();
        assert!(t.recent().is_empty());
        assert_eq!(t.len(), 0);
    }
}
