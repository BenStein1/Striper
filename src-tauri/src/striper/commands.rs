//! Tauri commands for the vocabulary manager and correction settings.

use super::StriperState;
use crate::settings;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::sync::Arc;
use striper_core::session::LearnRequest;
use striper_core::store::{EntryUpdate, VocabularyExport};
use striper_core::vocabulary::{MatchPolicy, Scope, VocabularyEntry};
use tauri::{AppHandle, State};

/// A learned spelling as shown in the vocabulary manager.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct LearnedEntry {
    pub id: i64,
    pub preferred_text: String,
    pub aliases: Vec<String>,
    pub context_words: Vec<String>,
    pub negative_context_words: Vec<String>,
    /// Replace aliases everywhere instead of only in supporting context.
    pub always: bool,
    /// Only apply in this application (lowercased executable name).
    pub app_scope: Option<String>,
    pub enabled: bool,
    pub use_as_asr_hint: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<VocabularyEntry> for LearnedEntry {
    fn from(e: VocabularyEntry) -> Self {
        Self {
            id: e.id,
            preferred_text: e.preferred_text,
            aliases: e.aliases,
            context_words: e.context_words,
            negative_context_words: e.negative_context_words,
            always: e.policy == MatchPolicy::Always,
            app_scope: match e.scope {
                Scope::Global => None,
                Scope::App(a) => Some(a),
            },
            enabled: e.enabled,
            use_as_asr_hint: e.use_as_asr_hint,
            created_at: e.created_at,
            updated_at: e.updated_at,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct LearnedEntryUpdate {
    pub preferred_text: Option<String>,
    pub aliases: Option<Vec<String>>,
    pub context_words: Option<Vec<String>>,
    pub negative_context_words: Option<Vec<String>>,
    pub always: Option<bool>,
    /// `Some("")` clears the app scope.
    pub app_scope: Option<String>,
    pub enabled: Option<bool>,
    pub use_as_asr_hint: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CorrectionRecord {
    pub id: i64,
    pub timestamp: i64,
    pub app_id: Option<String>,
    pub operation: String,
    pub instruction: String,
    pub before_text: String,
    pub after_text: String,
    pub result: String,
    pub interpreter: String,
}

type Striper<'a> = State<'a, Arc<StriperState>>;

fn split_words(items: Vec<String>) -> Vec<String> {
    items
        .into_iter()
        .flat_map(|s| {
            s.split(',')
                .map(|p| p.trim().to_string())
                .collect::<Vec<_>>()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

#[tauri::command]
#[specta::specta]
pub fn get_learned_vocabulary(state: Striper<'_>) -> Result<Vec<LearnedEntry>, String> {
    state
        .with_store(|s| s.list_entries())?
        .map(|v| v.into_iter().map(LearnedEntry::from).collect())
        .map_err(|e| e.to_string())
}

/// Add a spelling from the vocabulary manager (an explicit user action, so
/// it is confirmed immediately).
#[tauri::command]
#[specta::specta]
pub fn add_learned_vocabulary(
    state: Striper<'_>,
    preferred_text: String,
    aliases: Vec<String>,
    context_words: Vec<String>,
    always: bool,
) -> Result<LearnedEntry, String> {
    let request = LearnRequest {
        preferred_text,
        aliases: split_words(aliases),
        context_words: split_words(context_words),
        always,
        app_id: None,
    };
    let entry = state
        .with_store(|s| s.learn(&request).and_then(|id| s.get_entry(id)))?
        .map_err(|e| e.to_string())?;
    state.refresh_vocabulary();
    Ok(entry.into())
}

#[tauri::command]
#[specta::specta]
pub fn update_learned_vocabulary(
    state: Striper<'_>,
    id: i64,
    update: LearnedEntryUpdate,
) -> Result<LearnedEntry, String> {
    let core = EntryUpdate {
        preferred_text: update.preferred_text,
        aliases: update.aliases.map(split_words),
        policy: update.always.map(|a| {
            if a {
                MatchPolicy::Always
            } else {
                MatchPolicy::Contextual
            }
        }),
        context_words: update.context_words.map(split_words),
        negative_context_words: update.negative_context_words.map(split_words),
        scope: update.app_scope.map(|a| {
            let a = a.trim().to_lowercase();
            if a.is_empty() {
                Scope::Global
            } else {
                Scope::App(a)
            }
        }),
        enabled: update.enabled,
        use_as_asr_hint: update.use_as_asr_hint,
    };
    let entry = state
        .with_store(|s| s.update_entry(id, &core))?
        .map_err(|e| e.to_string())?;
    state.refresh_vocabulary();
    Ok(entry.into())
}

#[tauri::command]
#[specta::specta]
pub fn delete_learned_vocabulary(state: Striper<'_>, id: i64) -> Result<(), String> {
    state
        .with_store(|s| s.delete_entry(id))?
        .map_err(|e| e.to_string())?;
    state.refresh_vocabulary();
    Ok(())
}

/// Largest vocabulary file accepted for import.
const MAX_IMPORT_BYTES: u64 = 4 * 1024 * 1024;

/// Writes the learned vocabulary as JSON to `path` (chosen by the user in a
/// save dialog). Returns the number of entries written.
#[tauri::command]
#[specta::specta]
pub fn export_learned_vocabulary(state: Striper<'_>, path: String) -> Result<u32, String> {
    let export = state
        .with_store(|s| s.export())?
        .map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(&export).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("Could not write {path}: {e}"))?;
    Ok(export.entries.len() as u32)
}

/// Merges a vocabulary JSON file at `path` (chosen by the user in an open
/// dialog) into the store. Returns the number of entries imported.
#[tauri::command]
#[specta::specta]
pub fn import_learned_vocabulary(state: Striper<'_>, path: String) -> Result<u32, String> {
    let size = std::fs::metadata(&path)
        .map_err(|e| format!("Could not read {path}: {e}"))?
        .len();
    if size > MAX_IMPORT_BYTES {
        return Err("The file is too large to be a vocabulary export".into());
    }
    let json = std::fs::read_to_string(&path).map_err(|e| format!("Could not read {path}: {e}"))?;
    let data: VocabularyExport =
        serde_json::from_str(&json).map_err(|e| format!("Not a vocabulary export: {e}"))?;
    let n = state
        .with_store(|s| s.import(&data))?
        .map_err(|e| e.to_string())?;
    state.refresh_vocabulary();
    Ok(n as u32)
}

#[tauri::command]
#[specta::specta]
pub fn get_recent_corrections(
    state: Striper<'_>,
    limit: u32,
) -> Result<Vec<CorrectionRecord>, String> {
    let rows = state
        .with_store(|s| s.recent_transactions(limit.min(500) as usize))?
        .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|t| CorrectionRecord {
            id: t.id,
            timestamp: t.timestamp,
            app_id: t.app_id,
            operation: t.operation,
            instruction: t.instruction,
            before_text: t.before_text,
            after_text: t.after_text,
            result: t.result,
            interpreter: t.interpreter,
        })
        .collect())
}

#[tauri::command]
#[specta::specta]
pub fn change_learned_vocabulary_enabled_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    let mut s = settings::get_settings(&app);
    s.learned_vocabulary_enabled = enabled;
    settings::write_settings(&app, s);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_correction_llm_enabled_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut s = settings::get_settings(&app);
    s.correction_llm_enabled = enabled;
    settings::write_settings(&app, s);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_correction_require_readback_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    let mut s = settings::get_settings(&app);
    s.correction_require_readback = enabled;
    settings::write_settings(&app, s);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_correction_reasoning_effort_setting(
    app: AppHandle,
    state: State<'_, Arc<StriperState>>,
    effort: Option<String>,
) -> Result<(), String> {
    if let Some(ref value) = effort {
        if !["none", "low", "medium", "high", "xhigh", "max"].contains(&value.as_str()) {
            return Err("Unsupported reasoning effort".into());
        }
    }
    let mut s = settings::get_settings(&app);
    s.correction_reasoning_effort = effort;
    settings::write_settings(&app, s);
    state
        .llm_failures
        .store(0, std::sync::atomic::Ordering::SeqCst);
    state
        .llm_paused_until_ms
        .store(0, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

/// Configure the requested model without changing or exposing any API keys.
#[tauri::command]
#[specta::specta]
pub fn use_luna_correction_preset(
    app: AppHandle,
    state: State<'_, Arc<StriperState>>,
) -> Result<(), String> {
    let mut s = settings::get_settings(&app);
    s.post_process_provider_id = "openai".into();
    s.post_process_models
        .insert("openai".into(), "gpt-6-luna".into());
    s.correction_provider_id = None;
    s.correction_model = None;
    s.correction_llm_enabled = true;
    s.correction_reasoning_effort = Some("medium".into());
    settings::write_settings(&app, s);
    state
        .llm_failures
        .store(0, std::sync::atomic::Ordering::SeqCst);
    state
        .llm_paused_until_ms
        .store(0, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

/// Exercise the real provider with synthetic text; never touch a desktop target.
#[tauri::command]
#[specta::specta]
pub async fn test_correction_ai(
    app: AppHandle,
    state: State<'_, Arc<StriperState>>,
) -> Result<String, String> {
    let (_, _, model) = settings::get_settings(&app)
        .correction_llm()
        .ok_or("Choose an AI provider and model first.")?;
    state
        .llm_failures
        .store(0, std::sync::atomic::Ordering::SeqCst);
    state
        .llm_paused_until_ms
        .store(0, std::sync::atomic::Ordering::SeqCst);
    let ops = super::correction::interpret_with_llm(
        &app,
        &state,
        "Make this less formal for my son. Keep the same plans.",
        "I would be delighted to accompany you to lunch tomorrow.",
        "I would be delighted to accompany you to lunch tomorrow.",
        &[],
    )
    .await?;
    if !ops
        .iter()
        .any(striper_core::command::EditOperation::edits_text)
    {
        // Say what the model actually did, so a wrong model or a model that
        // keeps asking questions can be told apart.
        let answer = match ops.first() {
            Some(striper_core::command::EditOperation::Clarify { question }) => {
                format!("asked \u{201C}{question}\u{201D}")
            }
            _ => format!(
                "answered with {}",
                ops.iter()
                    .map(striper_core::command::EditOperation::name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        log::warn!("AI correction test: {model} {answer} instead of revising the sample");
        return Err(format!(
            "The provider replied, but {model} {answer} instead of revising the sample sentence."
        ));
    }
    let mut revised = "I would be delighted to accompany you to lunch tomorrow.".to_string();
    for op in &ops {
        revised = striper_core::edit::resolve_operation(&revised, op)
            .map_err(|e| format!("The provider returned an unusable sample edit: {e}"))?
            .new_text;
    }
    Ok(format!("{model}: {revised}"))
}
