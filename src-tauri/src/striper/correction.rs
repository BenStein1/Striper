//! Lane B: a spoken correction instruction → validated operations →
//! verified edits of the most recent dictation (PLAN.md §5).
//!
//! The instruction is recorded and transcribed by the same capture and
//! recognizer as dictation (the "correct" binding). Interpretation is local
//! and deterministic first; only unrecognised instructions go to the
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
use tauri::{AppHandle, Emitter, Manager};

const LLM_TIMEOUT: Duration = Duration::from_secs(15);
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
struct BusyGuard<'a>(&'a StriperState);

impl Drop for BusyGuard<'_> {
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

/// Ask the configured LLM to interpret an instruction the local parser did
/// not recognise. Returns validated operations or a user-facing reason.
async fn interpret_with_llm(
    app: &AppHandle,
    state: &StriperState,
    instruction: &str,
    region: &str,
    last_dictation: &str,
    previous: &[striper_core::parser::LastEdit],
) -> Result<Vec<EditOperation>, String> {
    let settings = crate::settings::get_settings(app);
    let Some((provider, api_key, model)) = settings.correction_llm() else {
        return Err("I didn't understand that. Try \"change Thursday to Friday\", \"scratch that\" or \"undo that\". (No AI provider is configured for other instructions.)".into());
    };
    if now_ms() < state.llm_paused_until_ms.load(Ordering::SeqCst) {
        return Err(
            "I didn't understand that, and the AI interpreter is paused after repeated errors."
                .into(),
        );
    }

    let user = build_user_message(instruction, region, last_dictation, previous);
    let system = system_prompt();
    let request = async {
        if provider.api_format == crate::settings::PostProcessApiFormat::Anthropic {
            crate::llm_client::send_chat_completion(
                &provider,
                api_key,
                &model,
                format!("{system}\n\n{user}"),
                true,
            )
            .await
        } else {
            let schema = provider
                .supports_structured_output
                .then(response_json_schema);
            crate::llm_client::send_chat_completion_with_schema(
                &provider,
                api_key,
                &model,
                user,
                Some(system),
                schema,
                true,
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
    let _busy = BusyGuard(&state);

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
    let (ops, interpreter) = match parse_instruction(&instruction, &parse_ctx) {
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
        let mut target = KeyboardTarget::new(&app, &settings);
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

    record_transactions(&state, &instruction, interpreter, &outcomes);
    let success = !outcomes.is_empty()
        && outcomes
            .iter()
            .all(|o| !matches!(o, StepOutcome::Failed { .. } | StepOutcome::Clarify { .. }));
    let ends = outcomes.iter().any(StepOutcome::ends_session);
    end_phase(&state, ends);
    let message = outcomes
        .iter()
        .map(StepOutcome::message)
        .collect::<Vec<_>>()
        .join(" ");
    finish(
        &app,
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
