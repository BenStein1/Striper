//! Striper's integration with the Handy pipeline.
//!
//! - Lane A: [`apply_learned_vocabulary`] runs on every transcript before it
//!   is inserted, and [`record_commit`] is the authoritative
//!   `TranscriptCommitted` hook (PLAN.md §3) called after each insertion.
//! - Lane B: [`correction`] turns a spoken instruction into verified edits of
//!   the most recent dictation.
//!
//! The decision logic lives in the `striper-core` crate; this module owns the
//! process-wide state and the platform glue.

pub mod commands;
pub mod correction;
mod keys;
mod target;

use log::{debug, error, info, warn};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use striper_core::session::{
    AppIdentity, CorrectionSession, DictationCommit, Ledger, LedgerConfig,
};
use striper_core::store::Store;
use striper_core::vocabulary::{apply_vocabulary, ApplyContext, VocabularyEntry};
use tauri::{AppHandle, Manager};

use crate::settings::{AppSettings, PasteMethod};

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Process-wide Striper state, managed by Tauri.
pub struct StriperState {
    store: Mutex<Store>,
    /// Active entries, cached so dictation never waits on the database.
    vocabulary: RwLock<Vec<VocabularyEntry>>,
    ledger: Mutex<Ledger>,
    session: Mutex<Option<CorrectionSession>>,
    /// A correction is being applied; a second one is refused.
    busy: AtomicBool,
    /// Consecutive LLM failures, and when the interpreter was paused.
    llm_failures: AtomicU32,
    llm_paused_until_ms: AtomicU64,
}

impl StriperState {
    fn new(store: Store) -> Self {
        let state = Self {
            store: Mutex::new(store),
            vocabulary: RwLock::new(Vec::new()),
            ledger: Mutex::new(Ledger::new(LedgerConfig::default())),
            session: Mutex::new(None),
            busy: AtomicBool::new(false),
            llm_failures: AtomicU32::new(0),
            llm_paused_until_ms: AtomicU64::new(0),
        };
        state.refresh_vocabulary();
        state
    }

    /// Reload the cached active entries after any vocabulary change.
    pub fn refresh_vocabulary(&self) {
        let entries = match self.store.lock() {
            Ok(store) => store.active_entries(),
            Err(_) => return,
        };
        match entries {
            Ok(entries) => {
                if let Ok(mut cache) = self.vocabulary.write() {
                    *cache = entries;
                }
            }
            Err(e) => error!("Failed to load learned vocabulary: {e}"),
        }
    }

    pub fn with_store<T>(&self, f: impl FnOnce(&Store) -> T) -> Result<T, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "vocabulary store is unavailable".to_string())?;
        Ok(f(&store))
    }
}

/// Open (or create) the Striper database and register the state. Normal
/// dictation must keep working even if this fails, so a broken database
/// falls back to an empty in-memory store.
pub fn init(app: &AppHandle) {
    let store = match crate::portable::app_data_dir(app) {
        Ok(dir) => {
            let path = dir.join("striper.db");
            match Store::open(&path) {
                Ok(store) => {
                    info!("Striper vocabulary database at {}", path.display());
                    Some(store)
                }
                Err(e) => {
                    error!(
                        "Could not open {}: {e}; learned vocabulary will not persist",
                        path.display()
                    );
                    None
                }
            }
        }
        Err(e) => {
            error!("No app data directory for the vocabulary database: {e}");
            None
        }
    };
    let store = store
        .unwrap_or_else(|| Store::open_in_memory().expect("in-memory SQLite is always available"));
    app.manage(Arc::new(StriperState::new(store)));
}

fn state(app: &AppHandle) -> Option<Arc<StriperState>> {
    app.try_state::<Arc<StriperState>>().map(|s| Arc::clone(&s))
}

/// Identity of the focused window right now.
pub fn focused_identity() -> AppIdentity {
    let w = striper_platform::foreground_window();
    debug!(
        "Focused window via {}: app={:?} id={:?}",
        w.source, w.app_id, w.window_id
    );
    AppIdentity {
        app_id: w.app_id,
        window_id: w.window_id,
        title: w.title,
    }
}

/// Lane A: apply learned spellings to a transcript before it is inserted.
/// Pure and in-memory; never blocks on the database or the network.
pub fn apply_learned_vocabulary(app: &AppHandle, text: &str, app_id: Option<&str>) -> String {
    let settings = crate::settings::get_settings(app);
    if !settings.learned_vocabulary_enabled || text.trim().is_empty() {
        return text.to_string();
    }
    let Some(state) = state(app) else {
        return text.to_string();
    };
    let Ok(entries) = state.vocabulary.read() else {
        return text.to_string();
    };
    if entries.is_empty() {
        return text.to_string();
    }
    let started = std::time::Instant::now();
    let outcome = apply_vocabulary(text, &entries, &ApplyContext { app_id });
    for rule in &outcome.applied {
        debug!(
            "Learned vocabulary: '{}' -> '{}' ({:?})",
            crate::utils::redact_text(&rule.original),
            crate::utils::redact_text(&rule.replacement),
            rule.reason
        );
    }
    debug!("Learned vocabulary applied in {:?}", started.elapsed());
    outcome.text
}

/// Preferred spellings to offer the recognizer as vocabulary hints.
pub fn asr_hints(app: &AppHandle) -> Vec<String> {
    let settings = crate::settings::get_settings(app);
    if !settings.learned_vocabulary_enabled {
        return Vec::new();
    }
    state(app)
        .and_then(|s| {
            s.vocabulary.read().ok().map(|v| {
                v.iter()
                    .filter(|e| e.use_as_asr_hint)
                    .map(|e| e.preferred_text.clone())
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// Exactly what an insertion types into the target app.
pub fn inserted_text(settings: &AppSettings, final_text: &str) -> String {
    if settings.append_trailing_space {
        format!("{final_text} ")
    } else {
        final_text.to_string()
    }
}

/// Whether text inserted with these settings can be edited afterwards:
/// it was typed/pasted at the caret, and no Enter followed it.
pub fn insertion_is_editable(settings: &AppSettings) -> bool {
    matches!(
        settings.paste_method,
        PasteMethod::CtrlV
            | PasteMethod::CtrlShiftV
            | PasteMethod::ShiftInsert
            | PasteMethod::Direct
    ) && !settings.auto_submit
}

/// The `TranscriptCommitted` hook: record a dictation that was just
/// inserted (or failed to insert) so it can be corrected.
pub fn record_commit(
    app: &AppHandle,
    raw_text: &str,
    final_text: &str,
    target: AppIdentity,
    inserted: bool,
) {
    let Some(state) = state(app) else {
        return;
    };
    let settings = crate::settings::get_settings(app);
    let Ok(mut ledger) = state.ledger.lock() else {
        return;
    };
    let id = ledger.next_commit_id();
    let editable = inserted && insertion_is_editable(&settings);
    if !editable {
        debug!(
            "Dictation {id} is not editable (method {:?})",
            settings.paste_method
        );
    }
    ledger.record_commit(DictationCommit {
        id,
        at_ms: now_ms(),
        target,
        raw_text: raw_text.to_string(),
        inserted_text: inserted_text(&settings, final_text),
        editable,
    });
    drop(ledger);
    // A new dictation ends any conversational session.
    if let Ok(mut session) = state.session.lock() {
        *session = None;
    };
}

/// Wait (briefly) until the user lets go of modifier keys, so synthetic
/// arrows are not turned into word jumps. Returns false if they stay held.
pub fn wait_for_modifier_release() -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
    loop {
        match striper_platform::modifiers_held() {
            Some(true) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Some(true) => {
                warn!("Modifier keys still held; refusing to send edit keystrokes");
                return false;
            }
            _ => return true,
        }
    }
}

pub(crate) use correction::handle_instruction;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailing_space_is_part_of_the_inserted_text() {
        let mut s = crate::settings::get_default_settings();
        assert_eq!(inserted_text(&s, "Hi."), "Hi.");
        s.append_trailing_space = true;
        assert_eq!(inserted_text(&s, "Hi."), "Hi. ");
    }

    #[test]
    fn only_caret_insertions_without_enter_are_editable() {
        let mut s = crate::settings::get_default_settings();
        s.paste_method = PasteMethod::CtrlV;
        s.auto_submit = false;
        assert!(insertion_is_editable(&s));
        s.auto_submit = true;
        assert!(!insertion_is_editable(&s));
        s.auto_submit = false;
        s.paste_method = PasteMethod::None;
        assert!(!insertion_is_editable(&s));
        s.paste_method = PasteMethod::ExternalScript;
        assert!(!insertion_is_editable(&s));
    }
}
