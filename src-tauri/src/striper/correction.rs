//! Lane B: a spoken correction instruction → validated operations →
//! verified edits of the most recent dictation (PLAN.md §5).
//!
//! The instruction is recorded and transcribed by the same capture and
//! recognizer as dictation (the "correct" binding). Interpretation is local
//! for session controls; natural-language edits go to the
//! configured OpenAI-compatible LLM, which can only return operations from
//! the closed set in `striper_core::command`. Execution then checks the
//! focused window, selects exactly the expected text, reads it back, and
//! only then replaces it.

use super::target::{ClipboardGuard, KeyboardTarget};
use super::{focused_identity, now_ms, wait_for_modifier_release, StriperState};
use crate::audio_feedback::{play_feedback_sound, SoundType};
use log::{debug, error, info, warn};
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use striper_core::command::{parse_command_response, response_json_schema, EditOperation};
use striper_core::edit::ExecuteOptions;
use striper_core::parser::{parse_instruction, ParseContext};
use striper_core::prompt::{build_user_message, system_prompt};
use striper_core::session::{
    run_operations, CorrectionSession, LearnRequest, RunContext, SessionPhase, StepOutcome,
    VocabularySink,
};
use striper_core::store::{now_secs, EditTransactionRecord};
use striper_platform::live_text::{FocusedText, LiveTextError};
use tauri::{AppHandle, Emitter, Manager};

const LLM_TIMEOUT: Duration = Duration::from_secs(45);
/// After this many consecutive interpreter failures, stop calling it for a while.
const LLM_FAILURE_LIMIT: u32 = 3;
const LLM_PAUSE_MS: u64 = 5 * 60 * 1000;

/// Result of one instruction, sent to the main window as `correction-result`.
#[derive(Debug, Clone, Serialize)]
pub struct CorrectionResult {
    pub instruction: String,
    pub interpreter: String,
    pub success: bool,
    pub message: String,
    pub outcomes: Vec<StepOutcome>,
}

/// Wraps the store so learning refreshes the in-memory vocabulary.
struct StateSink<'a>(&'a StriperState);

impl VocabularySink for StateSink<'_> {
    fn learn(&mut self, request: &LearnRequest) -> Result<i64, String> {
        let id = self
            .0
            .with_store(|s| s.learn(request).map_err(|e| e.to_string()))??;
        self.0.refresh_vocabulary();
        Ok(id)
    }

    fn forget(&mut self, text: Option<&str>) -> Result<Option<String>, String> {
        let r = self
            .0
            .with_store(|s| s.forget(text).map_err(|e| e.to_string()))??;
        self.0.refresh_vocabulary();
        Ok(r)
    }
}

/// Clears the busy flag however the instruction ends.
struct BusyGuard(Arc<StriperState>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::SeqCst);
    }
}

fn finish(app: &AppHandle, result: CorrectionResult) {
    let kind = if result.success {
        "success"
    } else if result
        .outcomes
        .iter()
        .any(|o| matches!(o, StepOutcome::Clarify { .. }))
    {
        "info"
    } else {
        "error"
    };
    info!(
        "Correction ({}): {} -> {}",
        result.interpreter,
        crate::utils::redact_text(&result.instruction),
        result.message
    );
    if result.success {
        play_feedback_sound(app, SoundType::Stop);
    }
    crate::utils::show_feedback_overlay(app, &result.message, kind);
    crate::tray::set_tray_state(app, crate::tray::TrayIconState::Idle);
    let _ = app.emit("correction-result", result);
}

fn simple_result(
    instruction: &str,
    interpreter: &str,
    success: bool,
    message: impl Into<String>,
) -> CorrectionResult {
    CorrectionResult {
        instruction: instruction.to_string(),
        interpreter: interpreter.to_string(),
        success,
        message: message.into(),
        outcomes: Vec::new(),
    }
}

/// Interpret natural-language edits with the configured AI. Returns validated
/// operations or a user-facing reason; never falls back after an AI failure.
pub(super) async fn interpret_with_llm(
    app: &AppHandle,
    state: &StriperState,
    instruction: &str,
    region: &str,
    last_dictation: &str,
    previous: &[striper_core::parser::LastEdit],
) -> Result<Vec<EditOperation>, String> {
    let user = build_user_message(instruction, region, last_dictation, previous);
    interpret_with_llm_message(app, state, user).await
}

/// Send a prepared interpreter request (ledger or live context).
pub(super) async fn interpret_with_llm_message(
    app: &AppHandle,
    state: &StriperState,
    user: String,
) -> Result<Vec<EditOperation>, String> {
    let settings = crate::settings::get_settings(app);
    let Some((provider, api_key, model)) = settings.correction_llm() else {
        return Err("Set up an AI provider and model under Vocabulary to use natural-language correction. Basic local commands and undo remain available.".into());
    };
    if now_ms() < state.llm_paused_until_ms.load(Ordering::SeqCst) {
        return Err(
            "AI correction is paused for five minutes after repeated errors. Check your provider settings; reselect the reasoning setting to retry now."
                .into(),
        );
    }

    if provider.api_format == crate::settings::PostProcessApiFormat::Anthropic
        && settings.correction_reasoning_effort.is_some()
    {
        return Err("Select Provider default reasoning for Anthropic-format endpoints.".into());
    }
    let system = system_prompt();
    let request = async {
        if provider.api_format == crate::settings::PostProcessApiFormat::Anthropic {
            crate::llm_client::send_chat_completion(
                &provider,
                api_key,
                &model,
                format!("{system}\n\n{user}"),
                false,
            )
            .await
        } else {
            let schema = provider
                .supports_structured_output
                .then(response_json_schema);
            crate::llm_client::send_chat_completion_with_effort(
                &provider,
                api_key,
                &model,
                user,
                Some(system),
                schema,
                false,
                settings.correction_reasoning_effort.as_deref(),
            )
            .await
        }
    };
    let started = std::time::Instant::now();
    let response = tokio::time::timeout(LLM_TIMEOUT, request).await;
    debug!("Correction interpreter answered in {:?}", started.elapsed());

    let outcome = match response {
        Err(_) => Err("The AI interpreter took too long; nothing was changed.".to_string()),
        Ok(Err(e)) => Err(format!(
            "The AI interpreter failed ({e}); nothing was changed."
        )),
        Ok(Ok(None)) => {
            Err("The AI interpreter returned nothing; nothing was changed.".to_string())
        }
        Ok(Ok(Some(content))) => {
            let content = content.trim();
            let content = content
                .strip_prefix("<think>")
                .and_then(|r| r.split_once("</think>").map(|(_, rest)| rest.trim()))
                .unwrap_or(content);
            parse_command_response(content)
                .map(|r| r.operations)
                .map_err(|e| {
                    warn!("Rejected interpreter response: {e}");
                    format!("I couldn't safely interpret that ({e}); nothing was changed.")
                })
        }
    };
    match &outcome {
        Ok(_) => state.llm_failures.store(0, Ordering::SeqCst),
        Err(_) => {
            let n = state.llm_failures.fetch_add(1, Ordering::SeqCst) + 1;
            if n >= LLM_FAILURE_LIMIT {
                warn!("Pausing the correction interpreter after {n} consecutive failures");
                state
                    .llm_paused_until_ms
                    .store(now_ms() + LLM_PAUSE_MS, Ordering::SeqCst);
                state.llm_failures.store(0, Ordering::SeqCst);
            }
        }
    }
    outcome
}

/// Handle one transcribed correction instruction end to end.
pub async fn handle_instruction(app: AppHandle, instruction: String) {
    let generation = app
        .state::<Arc<crate::managers::audio::AudioRecordingManager>>()
        .cancel_generation();
    handle_instruction_guarded(app, instruction, generation, false).await;
}

pub async fn handle_instruction_guarded(
    app: AppHandle,
    instruction: String,
    generation: u64,
    listener: bool,
) {
    let cancelled = |app: &AppHandle| {
        app.state::<Arc<crate::managers::audio::AudioRecordingManager>>()
            .was_cancelled_since(generation)
            || (listener && !crate::settings::get_settings(app).correction_listener_enabled)
    };
    if cancelled(&app) {
        return;
    }
    let Some(state) = app.try_state::<Arc<StriperState>>().map(|s| Arc::clone(&s)) else {
        return;
    };
    let instruction = instruction.trim().to_string();
    if instruction.is_empty() {
        finish(
            &app,
            simple_result("", "local", false, "I didn't hear a correction."),
        );
        return;
    }
    if state.busy.swap(true, Ordering::SeqCst) {
        finish(
            &app,
            simple_result(
                &instruction,
                "local",
                false,
                "Still working on the last correction.",
            ),
        );
        return;
    }
    let _busy = BusyGuard(Arc::clone(&state));

    // Session bookkeeping: a stale session starts fresh.
    let now = now_ms();
    if let Ok(mut session) = state.session.lock() {
        let fresh = session.as_ref().is_none_or(|s| s.is_expired(now));
        if fresh {
            *session = Some(CorrectionSession::new(now));
        }
        if let Some(s) = session.as_mut() {
            s.phase = SessionPhase::Idle;
            s.transition(SessionPhase::Listening, now);
            s.transition(SessionPhase::Thinking, now);
        }
    }

    // The focused field's live text is the source of truth where the
    // platform can read it (PLAN.md §9a); otherwise use the ledger.
    let live = tauri::async_runtime::spawn_blocking(super::live_field::read)
        .await
        .unwrap_or_else(|e| Err(LiveTextError::Failed(e.to_string())));
    match live {
        Ok(field) if !field.text.trim().is_empty() => {
            handle_live(app, state, instruction, generation, listener, field).await;
            return;
        }
        Ok(_) => {}
        Err(LiveTextError::Protected) => {
            end_phase(&state, true);
            finish(
                &app,
                simple_result(
                    &instruction,
                    "local",
                    false,
                    LiveTextError::Protected.to_string(),
                ),
            );
            return;
        }
        Err(e) => debug!("Live field unavailable, using recent dictation: {e}"),
    }

    let (region, last_dictation, last_edit, previous) = match state.ledger.lock() {
        Ok(l) => (
            l.region_text().unwrap_or_default(),
            l.last_dictation().unwrap_or_default().to_string(),
            l.last_edit(),
            l.history()
                .iter()
                .map(|e| striper_core::parser::LastEdit {
                    replaced_text: e.replaced_text.clone(),
                    inserted_text: e.inserted_text.clone(),
                })
                .collect::<Vec<_>>(),
        ),
        Err(_) => return,
    };

    // Reject a stale/foreign target before sending recent text to a provider.
    // Keyboard-only memory commands still work without any dictated region.
    if !region.is_empty() {
        let current = focused_identity();
        let error = state
            .ledger
            .lock()
            .ok()
            .and_then(|l| l.check_target(&current, now_ms()).err());
        if let Some(error) = error {
            end_phase(&state, true);
            finish(
                &app,
                simple_result(&instruction, "local", false, error.to_string()),
            );
            return;
        }
    }
    let parse_ctx = ParseContext {
        region_text: &region,
        last_edit,
    };
    let parsed = parse_instruction(&instruction, &parse_ctx);
    let ai_configured = crate::settings::get_settings(&app)
        .correction_llm()
        .is_some();
    let local = striper_core::parser::local_operations_for_mode(parsed, ai_configured);
    let (ops, interpreter) = match local {
        Some(ops) => (ops, "local"),
        None => match interpret_with_llm(
            &app,
            &state,
            &instruction,
            &region,
            &last_dictation,
            &previous,
        )
        .await
        {
            Ok(ops) => (ops, "llm"),
            Err(message) => {
                end_phase(&state, false);
                finish(&app, simple_result(&instruction, "llm", false, message));
                return;
            }
        },
    };
    if cancelled(&app) {
        end_phase(&state, true);
        return;
    }
    debug!("Correction operations ({interpreter}): {ops:?}");

    run_ledger_ops(
        app,
        state,
        instruction,
        interpreter,
        ops,
        generation,
        listener,
    )
    .await;
}

/// Execute operations against Striper's record of recent dictation.
async fn run_ledger_ops(
    app: AppHandle,
    state: Arc<StriperState>,
    instruction: String,
    interpreter: &'static str,
    ops: Vec<EditOperation>,
    generation: u64,
    listener: bool,
) {
    let needs_keys = ops.iter().any(EditOperation::edits_text);
    let app_for_run = app.clone();
    let state_for_run = Arc::clone(&state);
    let instruction_for_run = instruction.clone();
    let run = tauri::async_runtime::spawn_blocking(move || {
        let app = app_for_run;
        let state = state_for_run;
        if app
            .state::<Arc<crate::managers::audio::AudioRecordingManager>>()
            .was_cancelled_since(generation)
            || (listener && !crate::settings::get_settings(&app).correction_listener_enabled)
        {
            return vec![StepOutcome::Failed {
                message: "Correction cancelled; nothing was changed.".into(),
            }];
        }
        let settings = crate::settings::get_settings(&app);
        if needs_keys && !wait_for_modifier_release() {
            return vec![StepOutcome::Failed {
                message: "Please let go of Ctrl, Alt and Shift, then try again.".into(),
            }];
        }
        let current = if needs_keys {
            focused_identity()
        } else {
            Default::default()
        };
        let mut target = KeyboardTarget::new(&app, &settings, current.clone());
        let guard = needs_keys.then(|| ClipboardGuard::save(&app));
        let ctx = RunContext {
            instruction: &instruction_for_run,
            current_target: &current,
            now_ms: now_ms(),
            options: ExecuteOptions {
                require_readback: settings.correction_require_readback,
                verify_after: true,
            },
        };
        let outcomes = match state.ledger.lock() {
            Ok(mut ledger) => {
                run_operations(&mut ledger, &ops, &ctx, &mut target, &mut StateSink(&state))
            }
            Err(_) => vec![StepOutcome::Failed {
                message: "Internal error.".into(),
            }],
        };
        if let Some(guard) = guard {
            guard.restore(target.settle());
        }
        outcomes
    });
    let outcomes = match run.await {
        Ok(o) => o,
        Err(e) => {
            error!("Correction task failed: {e}");
            vec![StepOutcome::Failed {
                message: "Internal error while editing.".into(),
            }]
        }
    };

    conclude(&app, &state, instruction, interpreter, outcomes);
}

/// Live history entries kept for "undo that" and follow-ups.
const LIVE_HISTORY: usize = 20;

/// A correction against the focused field's live text: read → interpret →
/// verify unchanged (re-plan once) → apply → verify (PLAN.md §9a).
async fn handle_live(
    app: AppHandle,
    state: Arc<StriperState>,
    instruction: String,
    generation: u64,
    listener: bool,
    field: FocusedText,
) {
    use striper_core::live::{context_window, LiveEdit};
    use striper_core::prompt::{build_live_user_message, MAX_LIVE_CONTEXT_CHARS};
    let cancelled = |app: &AppHandle| {
        app.state::<Arc<crate::managers::audio::AudioRecordingManager>>()
            .was_cancelled_since(generation)
            || (listener && !crate::settings::get_settings(app).correction_listener_enabled)
    };
    let snapshot = super::live_field::to_snapshot(&field);
    let window = context_window(&snapshot, MAX_LIVE_CONTEXT_CHARS);
    let history: Vec<LiveEdit> = state
        .live_history
        .lock()
        .map(|h| h.clone())
        .unwrap_or_default();
    let previous: Vec<striper_core::parser::LastEdit> = history
        .iter()
        .map(|e| striper_core::parser::LastEdit {
            replaced_text: e.before.clone(),
            inserted_text: e.after.clone(),
        })
        .collect();
    let last_dictation = state
        .ledger
        .lock()
        .ok()
        .and_then(|l| l.last_dictation().map(str::to_string))
        .filter(|d| window.text.contains(d.trim()))
        .unwrap_or_default();
    let parse_ctx = ParseContext {
        region_text: &window.text,
        last_edit: previous.last().cloned(),
    };
    let parsed = parse_instruction(&instruction, &parse_ctx);
    let ai_configured = crate::settings::get_settings(&app)
        .correction_llm()
        .is_some();
    let local = striper_core::parser::local_operations_for_mode(parsed, ai_configured);
    let (ops, interpreter) = match local {
        Some(ops) => (ops, "local"),
        None => {
            let user = build_live_user_message(
                &instruction,
                &window.text,
                !window.complete,
                snapshot.selected_text(),
                &last_dictation,
                &previous,
            );
            match interpret_with_llm_message(&app, &state, user).await {
                Ok(ops) => (ops, "llm"),
                Err(message) => {
                    end_phase(&state, false);
                    finish(&app, simple_result(&instruction, "llm", false, message));
                    return;
                }
            }
        }
    };
    if cancelled(&app) {
        end_phase(&state, true);
        return;
    }
    debug!("Live correction operations ({interpreter}): {ops:?}");

    // "Scratch that" needs Striper's own dictation record.
    if ops
        .iter()
        .any(|op| matches!(op, EditOperation::DeleteRecentDictation))
        || (ops
            .iter()
            .any(|op| matches!(op, EditOperation::UndoLastEdit))
            && history.is_empty())
    {
        run_ledger_ops(
            app,
            state,
            instruction,
            interpreter,
            ops,
            generation,
            listener,
        )
        .await;
        return;
    }

    let app_for_run = app.clone();
    let state_for_run = Arc::clone(&state);
    let instruction_for_run = instruction.clone();
    let run = tauri::async_runtime::spawn_blocking(move || {
        let app = app_for_run;
        let state = state_for_run;
        if ops.iter().any(|op| matches!(op, EditOperation::Cancel)) {
            return vec![StepOutcome::Cancelled];
        }
        if let Some(EditOperation::Clarify { question }) = ops
            .iter()
            .find(|op| matches!(op, EditOperation::Clarify { .. }))
        {
            return vec![StepOutcome::Clarify {
                question: question.clone(),
            }];
        }
        let settings = crate::settings::get_settings(&app);
        let needs_keys = ops.iter().any(EditOperation::edits_text);
        if needs_keys && !wait_for_modifier_release() {
            return vec![StepOutcome::Failed {
                message: "Please let go of Ctrl, Alt and Shift, then try again.".into(),
            }];
        }
        let current = focused_identity();
        let keys = KeyboardTarget::new(&app, &settings, current.clone());
        let mut field = super::live_field::AccessibleField::new(keys);
        let guard = needs_keys.then(|| ClipboardGuard::save(&app));
        let require = settings.correction_require_readback;
        let mut outcomes = Vec::new();
        for op in &ops {
            let result = match op {
                EditOperation::UndoLastEdit => {
                    let last = state.live_history.lock().ok().and_then(|mut h| h.pop());
                    match last {
                        Some(edit) => striper_core::live::undo(&mut field, &edit, require)
                            .map(|done| (done, true))
                            .inspect_err(|_| {
                                // Keep it for another try.
                                if let Ok(mut h) = state.live_history.lock() {
                                    h.push(edit);
                                }
                            }),
                        None => Err(striper_core::live::LiveError::NotLive),
                    }
                }
                op if striper_core::live::is_live_operation(op) => striper_core::live::run(
                    &mut field,
                    std::slice::from_ref(op),
                    MAX_LIVE_CONTEXT_CHARS,
                    require,
                )
                .pop()
                .unwrap_or(Err(striper_core::live::LiveError::NotLive))
                .map(|done| (done, false)),
                other => {
                    // Memory and session controls don't touch the field.
                    let ctx = RunContext {
                        instruction: &instruction_for_run,
                        current_target: &current,
                        now_ms: now_ms(),
                        options: ExecuteOptions::default(),
                    };
                    let mut step = match state.ledger.lock() {
                        Ok(mut ledger) => run_operations(
                            &mut ledger,
                            std::slice::from_ref(other),
                            &ctx,
                            &mut NoKeys,
                            &mut StateSink(&state),
                        ),
                        Err(_) => vec![StepOutcome::Failed {
                            message: "Internal error.".into(),
                        }],
                    };
                    let outcome = step.pop().unwrap_or(StepOutcome::Failed {
                        message: "Internal error.".into(),
                    });
                    let stop =
                        matches!(outcome, StepOutcome::Failed { .. }) || outcome.ends_session();
                    outcomes.push(outcome);
                    if stop {
                        break;
                    }
                    continue;
                }
            };
            match result {
                Ok((edit, was_undo)) => {
                    outcomes.push(StepOutcome::Edited {
                        operation: edit.operation.clone(),
                        before: edit.before.clone(),
                        after: edit.after.clone(),
                    });
                    if !was_undo {
                        if let Ok(mut h) = state.live_history.lock() {
                            h.push(edit);
                            let excess = h.len().saturating_sub(LIVE_HISTORY);
                            h.drain(..excess);
                        }
                    }
                    // The caret-relative ledger no longer describes the field.
                    if let Ok(mut ledger) = state.ledger.lock() {
                        ledger.invalidate();
                    }
                }
                Err(e) => {
                    outcomes.push(StepOutcome::Failed {
                        message: e.to_string(),
                    });
                    break;
                }
            }
        }
        if let Some(guard) = guard {
            guard.restore(field.keys().settle());
        }
        outcomes
    });
    let outcomes = match run.await {
        Ok(o) => o,
        Err(e) => {
            error!("Live correction task failed: {e}");
            vec![StepOutcome::Failed {
                message: "Internal error while editing.".into(),
            }]
        }
    };
    conclude(&app, &state, instruction, interpreter, outcomes);
}

/// A target that refuses every keystroke: for operations that must not
/// touch the field (memory, session controls).
struct NoKeys;

impl striper_core::edit::TextTarget for NoKeys {
    fn move_left(&mut self, _: usize) -> Result<(), String> {
        Err("not a text edit".into())
    }
    fn move_right(&mut self, _: usize) -> Result<(), String> {
        Err("not a text edit".into())
    }
    fn select_left(&mut self, _: usize) -> Result<(), String> {
        Err("not a text edit".into())
    }
    fn read_selection(&mut self) -> Result<Option<String>, String> {
        Err("not a text edit".into())
    }
    fn replace_selection(&mut self, _: &str) -> Result<(), String> {
        Err("not a text edit".into())
    }
    fn delete_selection(&mut self) -> Result<(), String> {
        Err("not a text edit".into())
    }
    fn collapse_right(&mut self) -> Result<(), String> {
        Err("not a text edit".into())
    }
    fn undo_once(&mut self) -> Result<(), String> {
        Err("not a text edit".into())
    }
}

fn conclude(
    app: &AppHandle,
    state: &StriperState,
    instruction: String,
    interpreter: &str,
    outcomes: Vec<StepOutcome>,
) {
    record_transactions(state, &instruction, interpreter, &outcomes);
    let success = !outcomes.is_empty()
        && outcomes
            .iter()
            .all(|o| !matches!(o, StepOutcome::Failed { .. } | StepOutcome::Clarify { .. }));
    let ends = outcomes.iter().any(StepOutcome::ends_session);
    end_phase(state, ends);
    let message = outcomes
        .iter()
        .map(StepOutcome::message)
        .collect::<Vec<_>>()
        .join(" ");
    finish(
        app,
        CorrectionResult {
            instruction,
            interpreter: interpreter.to_string(),
            success,
            message,
            outcomes,
        },
    );
}

fn end_phase(state: &StriperState, end_session: bool) {
    if let Ok(mut session) = state.session.lock() {
        if end_session {
            *session = None;
        } else if let Some(s) = session.as_mut() {
            s.phase = SessionPhase::Idle;
            s.last_activity_ms = now_ms();
        }
    }
}

fn record_transactions(
    state: &StriperState,
    instruction: &str,
    interpreter: &str,
    outcomes: &[StepOutcome],
) {
    let app_id = state
        .ledger
        .lock()
        .ok()
        .and_then(|l| l.target().and_then(|t| t.app_id.clone()));
    let _ = state.with_store(|store| {
        for o in outcomes {
            let (operation, before, after, result) = match o {
                StepOutcome::Edited {
                    operation,
                    before,
                    after,
                } => (
                    operation.clone(),
                    before.clone(),
                    after.clone(),
                    "applied".to_string(),
                ),
                StepOutcome::Failed { message } => (
                    "failed".into(),
                    String::new(),
                    String::new(),
                    message.clone(),
                ),
                StepOutcome::Learned { preferred_text, .. } => (
                    "remember_spelling".into(),
                    String::new(),
                    preferred_text.clone(),
                    "learned".into(),
                ),
                StepOutcome::Forgot { preferred_text } => (
                    "forget_spelling".into(),
                    preferred_text.clone().unwrap_or_default(),
                    String::new(),
                    "forgot".into(),
                ),
                _ => continue,
            };
            if let Err(e) = store.record_transaction(&EditTransactionRecord {
                id: 0,
                timestamp: now_secs(),
                app_id: app_id.clone(),
                operation,
                instruction: instruction.to_string(),
                before_text: before,
                after_text: after,
                result,
                interpreter: interpreter.to_string(),
            }) {
                warn!("Failed to record edit transaction: {e}");
            }
        }
    });
}
