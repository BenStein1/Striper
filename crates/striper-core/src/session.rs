//! The dictation ledger (what Striper inserted, where) and the
//! conversational correction session built on it.
//!
//! The ledger is the authoritative record PLAN.md §3 asks for: every
//! committed dictation arrives here with its target identity, and only text
//! recorded here can be edited. Consecutive dictations into the same window
//! form one editable *region*; a dictation elsewhere, a non-editable
//! insertion, or expiry resets it.

use crate::command::{instruction_requests_memory, EditOperation};
use crate::edit::{
    execute_plan, plan_edit, resolve_operation, EditPlan, ExecuteError, ExecuteOptions,
    ResolveError, TextTarget,
};
use crate::parser::LastEdit;
use crate::text::fold;
use crate::vocabulary::learn_context_words;
use serde::{Deserialize, Serialize};

/// Identity of the window text was inserted into. Any field may be unknown
/// (e.g. on Wayland, where focus cannot be inspected).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppIdentity {
    /// Lowercased executable name ("notepad.exe", "kate").
    pub app_id: Option<String>,
    /// Opaque window handle (HWND, X11 window id).
    pub window_id: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetMatch {
    Same,
    Different,
    /// Not enough information to tell.
    Unknown,
}

impl AppIdentity {
    /// A known window must remain observable and equal throughout an edit.
    /// When a desktop never exposes window IDs, retain the explicit-hotkey
    /// path's application/readback fallback rather than inventing an identity.
    pub fn permits_continuation(&self, current: &AppIdentity) -> bool {
        if let Some(expected) = self.window_id.as_deref() {
            return current.window_id.as_deref() == Some(expected);
        }
        self.compare(current) != TargetMatch::Different
    }

    pub fn compare(&self, other: &AppIdentity) -> TargetMatch {
        if let (Some(a), Some(b)) = (&self.window_id, &other.window_id) {
            return if a == b {
                TargetMatch::Same
            } else {
                TargetMatch::Different
            };
        }
        if let (Some(a), Some(b)) = (&self.app_id, &other.app_id) {
            return if a.eq_ignore_ascii_case(b) {
                TargetMatch::Same
            } else {
                TargetMatch::Different
            };
        }
        TargetMatch::Unknown
    }
}

/// One dictation as committed to the focused application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictationCommit {
    pub id: u64,
    pub at_ms: u64,
    pub target: AppIdentity,
    /// Recognizer output before vocabulary/LLM cleanup.
    pub raw_text: String,
    /// Exactly what was typed/pasted, including any trailing space.
    pub inserted_text: String,
    /// False when the insertion cannot be safely edited afterwards (no
    /// paste, external script, auto-submit pressed Enter, …).
    pub editable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Segment {
    commit_id: u64,
    text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Region {
    target: AppIdentity,
    segments: Vec<Segment>,
    updated_at_ms: u64,
}

impl Region {
    fn text(&self) -> String {
        self.segments.iter().map(|s| s.text.as_str()).collect()
    }

    /// Rewrite segment texts after the region changed from `old` to `new`.
    /// Segments the change touches are merged into one; the rest keep their
    /// identity so "scratch that" still removes exactly the last dictation.
    fn retext(&mut self, old: &str, new: &str) {
        let prefix = common_prefix_bytes(old, new);
        let suffix = common_suffix_bytes(&old[prefix..], &new[prefix..]);
        let old_end = old.len() - suffix;
        let new_mid = &new[prefix..new.len() - suffix];

        let mut bounds = Vec::with_capacity(self.segments.len());
        let mut pos = 0;
        for seg in &self.segments {
            bounds.push((pos, pos + seg.text.len()));
            pos += seg.text.len();
        }
        let touched: Vec<usize> = if old_end > prefix {
            (0..bounds.len())
                .filter(|&i| bounds[i].0 < old_end && bounds[i].1 > prefix)
                .collect()
        } else {
            // Pure insertion: attach to the segment it follows.
            (0..bounds.len())
                .find(|&i| bounds[i].0 < prefix && prefix <= bounds[i].1)
                .or(if bounds.is_empty() { None } else { Some(0) })
                .into_iter()
                .collect()
        };
        let (Some(&first), Some(&last)) = (touched.first(), touched.last()) else {
            return;
        };
        let head = &self.segments[first].text[..prefix - bounds[first].0];
        let tail = &self.segments[last].text[old_end - bounds[last].0..];
        let merged = Segment {
            commit_id: self.segments[last].commit_id,
            text: format!("{head}{new_mid}{tail}"),
        };
        self.segments.splice(first..=last, [merged]);
        self.segments.retain(|s| !s.text.is_empty());
        debug_assert_eq!(self.text(), new);
    }
}

fn common_prefix_bytes(a: &str, b: &str) -> usize {
    a.char_indices()
        .zip(b.chars())
        .find(|((_, x), y)| x != y)
        .map(|((i, _), _)| i)
        .unwrap_or_else(|| a.len().min(b.len()))
}

fn common_suffix_bytes(a: &str, b: &str) -> usize {
    let mut n = 0;
    for (x, y) in a.chars().rev().zip(b.chars().rev()) {
        if x != y {
            break;
        }
        n += x.len_utf8();
    }
    n.min(a.len()).min(b.len())
}

/// A successfully applied assistant edit, kept for undo and follow-ups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedEdit {
    pub operation: String,
    pub before: String,
    pub after: String,
    pub replaced_text: String,
    pub inserted_text: String,
    pub at_ms: u64,
    segments_before: Vec<Segment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerConfig {
    pub max_segments: usize,
    pub max_chars: usize,
    /// A region untouched for this long can no longer be edited.
    pub expiry_ms: u64,
}

impl Default for LedgerConfig {
    fn default() -> Self {
        Self {
            max_segments: 8,
            max_chars: 2000,
            expiry_ms: 10 * 60 * 1000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    NothingToEdit,
    Expired,
    FocusChanged,
    NothingToUndo,
    Resolve(ResolveError),
    Execute(ExecuteError),
    MemoryNotRequested,
    Storage(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::NothingToEdit => write!(f, "There's no recent dictation I can edit."),
            SessionError::Expired => write!(f, "That dictation is too old to edit safely."),
            SessionError::FocusChanged => write!(
                f,
                "You're in a different window from the dictation, so I left it alone."
            ),
            SessionError::NothingToUndo => write!(f, "There's no edit to undo."),
            SessionError::Resolve(e) => write!(f, "{e}"),
            SessionError::Execute(e) => write!(f, "{e}"),
            SessionError::MemoryNotRequested => write!(
                f,
                "I didn't save that spelling. Say \"remember that\" if you want me to keep it."
            ),
            SessionError::Storage(e) => write!(f, "Couldn't update the vocabulary: {e}"),
        }
    }
}

impl std::error::Error for SessionError {}

/// An edit computed but not yet executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedEdit {
    pub operation: String,
    pub before: String,
    pub after: String,
    pub plan: EditPlan,
    pub replaced_text: String,
    pub inserted_text: String,
    is_undo: bool,
}

/// Every dictation Striper committed recently, and the edits made to it.
#[derive(Debug, Clone, Default)]
pub struct Ledger {
    config: LedgerConfig,
    region: Option<Region>,
    history: Vec<AppliedEdit>,
    next_commit_id: u64,
}

impl Ledger {
    pub fn new(config: LedgerConfig) -> Self {
        Self {
            config,
            ..Default::default()
        }
    }

    pub fn next_commit_id(&mut self) -> u64 {
        self.next_commit_id += 1;
        self.next_commit_id
    }

    /// Record a committed dictation (the bridge's `TranscriptCommitted`).
    pub fn record_commit(&mut self, commit: DictationCommit) {
        if !commit.editable || commit.inserted_text.is_empty() {
            self.invalidate();
            return;
        }
        let continues = self.region.as_ref().is_some_and(|r| {
            r.target.compare(&commit.target) == TargetMatch::Same
                && commit.at_ms.saturating_sub(r.updated_at_ms) <= self.config.expiry_ms
        });
        if !continues {
            self.history.clear();
            self.region = Some(Region {
                target: commit.target.clone(),
                segments: Vec::new(),
                updated_at_ms: commit.at_ms,
            });
        }
        let region = self.region.as_mut().expect("region set above");
        region.segments.push(Segment {
            commit_id: commit.id,
            text: commit.inserted_text,
        });
        region.updated_at_ms = commit.at_ms;
        while region.segments.len() > self.config.max_segments
            || (region.segments.len() > 1 && region.text().chars().count() > self.config.max_chars)
        {
            region.segments.remove(0);
            // Undo snapshots refer to text that is no longer tracked.
            self.history.clear();
        }
    }

    /// Forget everything editable (focus moved, unsafe insertion, …).
    pub fn invalidate(&mut self) {
        self.region = None;
        self.history.clear();
    }

    pub fn region_text(&self) -> Option<String> {
        self.region.as_ref().map(Region::text)
    }

    pub fn last_dictation(&self) -> Option<&str> {
        self.region
            .as_ref()
            .and_then(|r| r.segments.last())
            .map(|s| s.text.as_str())
    }

    pub fn target(&self) -> Option<&AppIdentity> {
        self.region.as_ref().map(|r| &r.target)
    }

    pub fn history(&self) -> &[AppliedEdit] {
        &self.history
    }

    pub fn last_edit(&self) -> Option<LastEdit> {
        self.history.last().map(|e| LastEdit {
            replaced_text: e.replaced_text.clone(),
            inserted_text: e.inserted_text.clone(),
        })
    }

    /// Whether `current` (the focused window now) may be edited. Unknown
    /// identity is allowed only because some desktops (Wayland) never
    /// reveal it; the selection readback still guards the edit itself.
    pub fn check_target(&self, current: &AppIdentity, now_ms: u64) -> Result<(), SessionError> {
        let region = self.region.as_ref().ok_or(SessionError::NothingToEdit)?;
        if now_ms.saturating_sub(region.updated_at_ms) > self.config.expiry_ms {
            return Err(SessionError::Expired);
        }
        if region.target.compare(current) == TargetMatch::Different {
            return Err(SessionError::FocusChanged);
        }
        Ok(())
    }

    /// Compute the edit an operation makes, without executing it.
    pub fn plan(&self, op: &EditOperation) -> Result<PlannedEdit, SessionError> {
        let region = self.region.as_ref().ok_or(SessionError::NothingToEdit)?;
        let before = region.text();
        let (after, replaced_text, inserted_text, is_undo) = match op {
            EditOperation::DeleteRecentDictation => {
                let last = region.segments.last().ok_or(SessionError::NothingToEdit)?;
                let after = before[..before.len() - last.text.len()].to_string();
                (after, last.text.clone(), String::new(), false)
            }
            EditOperation::UndoLastEdit => {
                let last = self.history.last().ok_or(SessionError::NothingToUndo)?;
                (
                    last.before.clone(),
                    last.inserted_text.clone(),
                    last.replaced_text.clone(),
                    true,
                )
            }
            other => {
                let r = resolve_operation(&before, other).map_err(SessionError::Resolve)?;
                (r.new_text, r.replaced_text, r.inserted_text, false)
            }
        };
        let plan =
            plan_edit(&before, &after).ok_or(SessionError::Resolve(ResolveError::NoChange))?;
        Ok(PlannedEdit {
            operation: op.name().to_string(),
            before,
            after,
            plan,
            replaced_text,
            inserted_text,
            is_undo,
        })
    }

    /// Record a planned edit as applied.
    pub fn commit_edit(&mut self, planned: PlannedEdit, now_ms: u64) {
        let Some(region) = self.region.as_mut() else {
            return;
        };
        if planned.is_undo {
            if let Some(last) = self.history.pop() {
                region.segments = last.segments_before;
            }
        } else {
            let segments_before = region.segments.clone();
            if planned.operation == "delete_recent_dictation" {
                region.segments.pop();
            } else {
                region.retext(&planned.before, &planned.after);
            }
            self.history.push(AppliedEdit {
                operation: planned.operation,
                before: planned.before,
                after: planned.after,
                replaced_text: planned.replaced_text,
                inserted_text: planned.inserted_text,
                at_ms: now_ms,
                segments_before,
            });
        }
        region.updated_at_ms = now_ms;
    }
}

/// A spelling the user asked Striper to remember.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnRequest {
    pub preferred_text: String,
    pub aliases: Vec<String>,
    pub context_words: Vec<String>,
    pub always: bool,
    pub app_id: Option<String>,
}

/// Where learned spellings go (the SQLite store in production).
pub trait VocabularySink {
    /// Persist a confirmed preference; returns the entry id.
    fn learn(&mut self, request: &LearnRequest) -> Result<i64, String>;
    /// Disable the entry matching `text` (preferred spelling or alias), or
    /// the most recently learned one. Returns the preferred text disabled.
    fn forget(&mut self, text: Option<&str>) -> Result<Option<String>, String>;
}

/// Correction session phases (PLAN.md §4 module 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPhase {
    Idle,
    Listening,
    Thinking,
    Applying,
}

/// One conversational correction session: a run of instructions that can
/// refer back to each other, ended by "done", "cancel", timeout or a
/// different window.
#[derive(Debug, Clone)]
pub struct CorrectionSession {
    pub phase: SessionPhase,
    pub started_at_ms: u64,
    pub last_activity_ms: u64,
    pub instructions: u32,
    pub timeout_ms: u64,
}

impl CorrectionSession {
    pub const DEFAULT_TIMEOUT_MS: u64 = 60_000;

    pub fn new(now_ms: u64) -> Self {
        Self {
            phase: SessionPhase::Idle,
            started_at_ms: now_ms,
            last_activity_ms: now_ms,
            instructions: 0,
            timeout_ms: Self::DEFAULT_TIMEOUT_MS,
        }
    }

    pub fn is_expired(&self, now_ms: u64) -> bool {
        self.phase == SessionPhase::Idle
            && now_ms.saturating_sub(self.last_activity_ms) > self.timeout_ms
    }

    /// Move to `next`; returns false (and stays put) for invalid moves.
    pub fn transition(&mut self, next: SessionPhase, now_ms: u64) -> bool {
        use SessionPhase::*;
        let ok = matches!(
            (self.phase, next),
            (Idle, Listening)
                | (Listening, Thinking)
                | (Listening, Idle)
                | (Thinking, Applying)
                | (Thinking, Idle)
                | (Applying, Idle)
        );
        if ok {
            if next == Thinking {
                self.instructions += 1;
            }
            self.phase = next;
            self.last_activity_ms = now_ms;
        }
        ok
    }
}

/// What happened to one operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepOutcome {
    Edited {
        operation: String,
        before: String,
        after: String,
    },
    Learned {
        preferred_text: String,
        entry_id: i64,
    },
    Forgot {
        preferred_text: Option<String>,
    },
    Clarify {
        question: String,
    },
    Finished,
    Cancelled,
    Failed {
        message: String,
    },
}

impl StepOutcome {
    pub fn ends_session(&self) -> bool {
        matches!(self, StepOutcome::Finished | StepOutcome::Cancelled)
    }

    /// A short, plain-language message for the overlay / screen reader.
    pub fn message(&self) -> String {
        match self {
            StepOutcome::Edited { operation, .. } => match operation.as_str() {
                "undo_last_edit" => "Undone.".into(),
                "delete_recent_dictation" => "Deleted the last dictation.".into(),
                _ => "Changed.".into(),
            },
            StepOutcome::Learned { preferred_text, .. } => {
                format!("I'll remember \"{preferred_text}\".")
            }
            StepOutcome::Forgot {
                preferred_text: Some(t),
            } => format!("Forgot \"{t}\"."),
            StepOutcome::Forgot {
                preferred_text: None,
            } => "There was no saved spelling to forget.".into(),
            StepOutcome::Clarify { question } => question.clone(),
            StepOutcome::Finished => "Done.".into(),
            StepOutcome::Cancelled => "Cancelled.".into(),
            StepOutcome::Failed { message } => message.clone(),
        }
    }
}

/// Everything needed to run one instruction's operations.
pub struct RunContext<'a> {
    pub instruction: &'a str,
    pub current_target: &'a AppIdentity,
    pub now_ms: u64,
    pub options: ExecuteOptions,
}

/// Execute validated operations in order, stopping at the first failure.
/// Text edits go through `target` with readback verification; memory
/// operations go to `vocab` and require the user's own request.
pub fn run_operations(
    ledger: &mut Ledger,
    ops: &[EditOperation],
    ctx: &RunContext<'_>,
    target: &mut dyn TextTarget,
    vocab: &mut dyn VocabularySink,
) -> Vec<StepOutcome> {
    // A model may return a control operation alongside edits. Cancellation
    // and unresolved ambiguity apply to the whole instruction, regardless of
    // their position; never make changes before asking what the user meant.
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
    let mut outcomes = Vec::new();
    // Region text as it was before this instruction: the misheard word
    // (and its context) lives there, not in the corrected text.
    let original_region = ledger.region_text().unwrap_or_default();
    for op in ops {
        let outcome = match op {
            EditOperation::Clarify { question } => StepOutcome::Clarify {
                question: question.clone(),
            },
            EditOperation::Finish => StepOutcome::Finished,
            EditOperation::Cancel => StepOutcome::Cancelled,
            EditOperation::RememberSpelling {
                preferred_text,
                aliases,
                context_words,
                always,
            } => {
                if !instruction_requests_memory(ctx.instruction) {
                    StepOutcome::Failed {
                        message: SessionError::MemoryNotRequested.to_string(),
                    }
                } else {
                    let request = build_learn_request(
                        &original_region,
                        preferred_text,
                        aliases,
                        context_words,
                        *always,
                        ledger.target().and_then(|t| t.app_id.clone()),
                    );
                    match vocab.learn(&request) {
                        Ok(entry_id) => StepOutcome::Learned {
                            preferred_text: request.preferred_text,
                            entry_id,
                        },
                        Err(e) => StepOutcome::Failed {
                            message: SessionError::Storage(e).to_string(),
                        },
                    }
                }
            }
            EditOperation::ForgetSpelling { text } => match vocab.forget(text.as_deref()) {
                Ok(preferred_text) => StepOutcome::Forgot { preferred_text },
                Err(e) => StepOutcome::Failed {
                    message: SessionError::Storage(e).to_string(),
                },
            },
            text_op => match apply_text_op(ledger, text_op, ctx, target) {
                Ok(edit) => edit,
                Err(e) => StepOutcome::Failed {
                    message: e.to_string(),
                },
            },
        };
        let stop = matches!(outcome, StepOutcome::Failed { .. }) || outcome.ends_session();
        outcomes.push(outcome);
        if stop {
            break;
        }
    }
    outcomes
}

fn apply_text_op(
    ledger: &mut Ledger,
    op: &EditOperation,
    ctx: &RunContext<'_>,
    target: &mut dyn TextTarget,
) -> Result<StepOutcome, SessionError> {
    ledger.check_target(ctx.current_target, ctx.now_ms)?;
    let planned = ledger.plan(op)?;
    match execute_plan(target, &planned.plan, ctx.options) {
        Ok(()) => {
            let outcome = StepOutcome::Edited {
                operation: planned.operation.clone(),
                before: planned.before.clone(),
                after: planned.after.clone(),
            };
            ledger.commit_edit(planned, ctx.now_ms);
            Ok(outcome)
        }
        Err(e) => {
            // After a failed or unverifiable change the region no longer
            // matches what we believe; never edit it again blindly.
            if matches!(
                e,
                ExecuteError::PostVerifyFailed { .. } | ExecuteError::Platform(_)
            ) {
                ledger.invalidate();
            }
            Err(SessionError::Execute(e))
        }
    }
}

fn build_learn_request(
    region: &str,
    preferred: &str,
    aliases: &[String],
    context_words: &[String],
    always: bool,
    app_id: Option<String>,
) -> LearnRequest {
    let preferred = preferred.trim().to_string();
    let mut aliases: Vec<String> = aliases
        .iter()
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty() && a != &preferred)
        .collect();
    aliases.dedup_by(|a, b| fold(a) == fold(b));
    let mut words: Vec<String> = context_words
        .iter()
        .map(|w| fold(w.trim()))
        .filter(|w| !w.is_empty() && !w.contains(' '))
        .collect();
    for alias in &aliases {
        for occurrence in crate::edit::find_occurrences(region, alias) {
            for w in learn_context_words(region, occurrence, &[alias.as_str(), preferred.as_str()])
            {
                if !words.contains(&w) {
                    words.push(w);
                }
            }
        }
    }
    words.truncate(12);
    LearnRequest {
        preferred_text: preferred,
        aliases,
        context_words: words,
        always,
        app_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::SimulatedField;
    use crate::parser::{parse_instruction, ParseContext};

    fn notepad() -> AppIdentity {
        AppIdentity {
            app_id: Some("notepad.exe".into()),
            window_id: Some("0x1".into()),
            title: None,
        }
    }

    fn commit(ledger: &mut Ledger, field: &mut SimulatedField, text: &str, at: u64) {
        let id = ledger.next_commit_id();
        field.replace_selection(text).unwrap();
        ledger.record_commit(DictationCommit {
            id,
            at_ms: at,
            target: notepad(),
            raw_text: text.into(),
            inserted_text: text.into(),
            editable: true,
        });
    }

    #[derive(Default)]
    struct MemoryVocab {
        learned: Vec<LearnRequest>,
        forgotten: Vec<Option<String>>,
    }

    impl VocabularySink for MemoryVocab {
        fn learn(&mut self, request: &LearnRequest) -> Result<i64, String> {
            self.learned.push(request.clone());
            Ok(self.learned.len() as i64)
        }
        fn forget(&mut self, text: Option<&str>) -> Result<Option<String>, String> {
            self.forgotten.push(text.map(str::to_string));
            Ok(Some(text.unwrap_or("Striper").to_string()))
        }
    }

    fn say(
        ledger: &mut Ledger,
        field: &mut SimulatedField,
        vocab: &mut MemoryVocab,
        instruction: &str,
        now: u64,
    ) -> Vec<StepOutcome> {
        let region = ledger.region_text().unwrap_or_default();
        let pctx = ParseContext {
            region_text: &region,
            last_edit: ledger.last_edit(),
        };
        let ops = parse_instruction(instruction, &pctx).expect("instruction should parse");
        let target = notepad();
        let ctx = RunContext {
            instruction,
            current_target: &target,
            now_ms: now,
            options: ExecuteOptions::default(),
        };
        run_operations(ledger, &ops, &ctx, field, vocab)
    }

    // PLAN.md M1 demo + §5 example table.
    #[test]
    fn thursday_friday_saturday_undo() {
        let mut ledger = Ledger::default();
        let mut field = SimulatedField::with_text("Dear Bob, ");
        let mut vocab = MemoryVocab::default();
        commit(&mut ledger, &mut field, "I'll see you on Thursday.", 1_000);

        say(
            &mut ledger,
            &mut field,
            &mut vocab,
            "Correct that. Change Thursday to Friday.",
            2_000,
        );
        assert_eq!(field.text(), "Dear Bob, I'll see you on Friday.");

        say(
            &mut ledger,
            &mut field,
            &mut vocab,
            "Actually, make that Saturday.",
            3_000,
        );
        assert_eq!(field.text(), "Dear Bob, I'll see you on Saturday.");

        say(&mut ledger, &mut field, &mut vocab, "Undo that.", 4_000);
        assert_eq!(field.text(), "Dear Bob, I'll see you on Friday.");
        say(&mut ledger, &mut field, &mut vocab, "Undo that.", 5_000);
        assert_eq!(field.text(), "Dear Bob, I'll see you on Thursday.");
        let out = say(&mut ledger, &mut field, &mut vocab, "Undo that.", 6_000);
        assert!(matches!(&out[0], StepOutcome::Failed { .. }));
    }

    #[test]
    fn edits_span_consecutive_dictations_and_scratch_removes_only_the_last() {
        let mut ledger = Ledger::default();
        let mut field = SimulatedField::with_text("");
        let mut vocab = MemoryVocab::default();
        commit(&mut ledger, &mut field, "See you on Thursday. ", 1_000);
        commit(&mut ledger, &mut field, "Bring the files. ", 2_000);
        say(
            &mut ledger,
            &mut field,
            &mut vocab,
            "change Thursday to Friday",
            3_000,
        );
        assert_eq!(field.text(), "See you on Friday. Bring the files. ");
        say(&mut ledger, &mut field, &mut vocab, "scratch that", 4_000);
        assert_eq!(field.text(), "See you on Friday. ");
        say(&mut ledger, &mut field, &mut vocab, "undo that", 5_000);
        assert_eq!(field.text(), "See you on Friday. Bring the files. ");
    }

    // PLAN.md §0: teach the app its own name, with memory requested.
    #[test]
    fn teaching_striper_edits_and_learns_with_context() {
        let mut ledger = Ledger::default();
        let mut field = SimulatedField::with_text("");
        let mut vocab = MemoryVocab::default();
        commit(
            &mut ledger,
            &mut field,
            "I opened striped to write emails.",
            1_000,
        );
        let out = say(
            &mut ledger,
            &mut field,
            &mut vocab,
            "Correct that; I meant Striper, S-T-R-I-P-E-R; remember that.",
            2_000,
        );
        assert_eq!(field.text(), "I opened Striper to write emails.");
        assert!(
            matches!(&out[1], StepOutcome::Learned { preferred_text, .. } if preferred_text == "Striper")
        );
        let learned = &vocab.learned[0];
        assert_eq!(learned.aliases, vec!["striped".to_string()]);
        assert!(learned.context_words.contains(&"opened".to_string()));
        assert!(learned.context_words.contains(&"write".to_string()));
        assert!(!learned.always);
    }

    #[test]
    fn ambiguity_or_cancel_prevents_all_edits_even_when_returned_last() {
        for control in [
            EditOperation::Clarify {
                question: "Which day?".into(),
            },
            EditOperation::Cancel,
        ] {
            let mut ledger = Ledger::default();
            let mut field = SimulatedField::with_text("");
            let mut vocab = MemoryVocab::default();
            commit(&mut ledger, &mut field, "hello", 1_000);
            let ops = vec![EditOperation::DeleteRecentDictation, control];
            let target = notepad();
            let ctx = RunContext {
                instruction: "change that",
                current_target: &target,
                now_ms: 2_000,
                options: ExecuteOptions::default(),
            };
            let out = run_operations(&mut ledger, &ops, &ctx, &mut field, &mut vocab);
            assert_eq!(out.len(), 1);
            assert!(matches!(
                &out[0],
                StepOutcome::Clarify { .. } | StepOutcome::Cancelled
            ));
            assert_eq!(field.text(), "hello");
            assert!(ledger.history().is_empty());
            assert!(vocab.learned.is_empty());
        }
    }

    #[test]
    fn llm_proposed_memory_needs_the_users_words() {
        let mut ledger = Ledger::default();
        let mut field = SimulatedField::with_text("");
        let mut vocab = MemoryVocab::default();
        commit(&mut ledger, &mut field, "hello", 1_000);
        let ops = vec![EditOperation::RememberSpelling {
            preferred_text: "Hello".into(),
            aliases: vec![],
            context_words: vec![],
            always: true,
        }];
        let target = notepad();
        let ctx = RunContext {
            instruction: "capitalize hello",
            current_target: &target,
            now_ms: 2_000,
            options: ExecuteOptions::default(),
        };
        let out = run_operations(&mut ledger, &ops, &ctx, &mut field, &mut vocab);
        assert!(matches!(&out[0], StepOutcome::Failed { .. }));
        assert!(vocab.learned.is_empty());
    }

    #[test]
    fn refuses_other_windows_and_stale_regions() {
        let mut ledger = Ledger::default();
        let mut field = SimulatedField::with_text("");
        commit(&mut ledger, &mut field, "on Thursday", 1_000);
        let word = AppIdentity {
            app_id: Some("winword.exe".into()),
            window_id: Some("0x2".into()),
            title: None,
        };
        assert_eq!(
            ledger.check_target(&word, 2_000),
            Err(SessionError::FocusChanged)
        );
        assert_eq!(ledger.check_target(&notepad(), 2_000), Ok(()));
        assert_eq!(ledger.check_target(&AppIdentity::default(), 2_000), Ok(()));
        assert_eq!(
            ledger.check_target(&notepad(), 1_000 + 11 * 60 * 1000),
            Err(SessionError::Expired)
        );
    }

    #[test]
    fn a_dictation_elsewhere_resets_the_region() {
        let mut ledger = Ledger::default();
        let mut field = SimulatedField::with_text("");
        commit(&mut ledger, &mut field, "first ", 1_000);
        let id = ledger.next_commit_id();
        ledger.record_commit(DictationCommit {
            id,
            at_ms: 2_000,
            target: AppIdentity {
                app_id: Some("kate".into()),
                window_id: Some("9".into()),
                title: None,
            },
            raw_text: "other".into(),
            inserted_text: "other".into(),
            editable: true,
        });
        assert_eq!(ledger.region_text().as_deref(), Some("other"));
    }

    #[test]
    fn non_editable_insertions_clear_the_region() {
        let mut ledger = Ledger::default();
        let mut field = SimulatedField::with_text("");
        commit(&mut ledger, &mut field, "text", 1_000);
        ledger.record_commit(DictationCommit {
            id: 9,
            at_ms: 2_000,
            target: notepad(),
            raw_text: "x".into(),
            inserted_text: "x".into(),
            editable: false,
        });
        assert_eq!(ledger.region_text(), None);
    }

    #[test]
    fn mismatch_aborts_without_touching_text() {
        let mut ledger = Ledger::default();
        let mut field = SimulatedField::with_text("");
        let mut vocab = MemoryVocab::default();
        commit(&mut ledger, &mut field, "on Thursday", 1_000);
        field.replace_selection(" and more typed by hand").unwrap();
        let out = say(
            &mut ledger,
            &mut field,
            &mut vocab,
            "change Thursday to Friday",
            2_000,
        );
        assert!(matches!(&out[0], StepOutcome::Failed { .. }));
        assert_eq!(field.text(), "on Thursday and more typed by hand");
    }

    #[test]
    fn region_is_bounded() {
        let mut ledger = Ledger::new(LedgerConfig {
            max_segments: 2,
            ..Default::default()
        });
        let mut field = SimulatedField::with_text("");
        for (i, t) in ["a ", "b ", "c "].iter().enumerate() {
            commit(&mut ledger, &mut field, t, 1_000 + i as u64);
        }
        assert_eq!(ledger.region_text().as_deref(), Some("b c "));
    }

    #[test]
    fn retext_keeps_segments_consistent() {
        let mut region = Region {
            target: AppIdentity::default(),
            segments: vec![
                Segment {
                    commit_id: 1,
                    text: "See you on Thursday. ".into(),
                },
                Segment {
                    commit_id: 2,
                    text: "Bring files. ".into(),
                },
            ],
            updated_at_ms: 0,
        };
        let old = region.text();
        let new = old.replace("Thursday", "Friday");
        region.retext(&old, &new);
        assert_eq!(region.segments.len(), 2);
        assert_eq!(region.segments[1].text, "Bring files. ");
        // An edit spanning the boundary merges the segments.
        let old = region.text();
        let new = "See you on Friday files. ".to_string();
        region.retext(&old, &new);
        assert_eq!(region.text(), new);
    }

    #[test]
    fn losing_a_previously_known_window_refuses_further_keystrokes() {
        let expected = notepad();
        let mut unknown_window = expected.clone();
        unknown_window.window_id = None;
        assert!(!expected.permits_continuation(&unknown_window));
        assert!(expected.permits_continuation(&expected));
        let mut different = expected.clone();
        different.window_id = Some("another-window".into());
        assert!(!expected.permits_continuation(&different));
        assert!(AppIdentity::default().permits_continuation(&AppIdentity::default()));
    }

    #[test]
    fn session_phases() {
        let mut s = CorrectionSession::new(0);
        assert!(s.transition(SessionPhase::Listening, 10));
        assert!(!s.transition(SessionPhase::Applying, 11));
        assert!(s.transition(SessionPhase::Thinking, 12));
        assert!(s.transition(SessionPhase::Applying, 13));
        assert!(s.transition(SessionPhase::Idle, 14));
        assert_eq!(s.instructions, 1);
        assert!(!s.is_expired(14 + 1_000));
        assert!(s.is_expired(14 + CorrectionSession::DEFAULT_TIMEOUT_MS + 1));
    }
}
