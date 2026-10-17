//! Corrections against the focused field's live text (PLAN.md §9a).
//!
//! The application's current text is the source of truth. One correction
//! runs: read the field → resolve operations against that text → read again
//! and make sure nothing changed while the interpreter was thinking (re-plan
//! once if it did) → select exactly the planned span, read it back, replace
//! it → read the field again and confirm the result, undoing on mismatch.
//!
//! Operations are content-addressed (`expected_text`, anchors), so a
//! re-plan after a concurrent change re-resolves the same operations against
//! the new text without asking the interpreter again.

use crate::command::EditOperation;
use crate::edit::{resolve_operation, ResolveError};
use crate::text::{common_prefix_len, common_suffix_len};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

/// How much of the field a snapshot covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldScope {
    /// Text read through an accessibility API: the whole field, or a window
    /// around the caret (see [`FieldSnapshot::whole_document`]).
    Field,
    /// Only the user's selection could be read; edits are confined to it.
    SelectionOnly,
}

/// The focused field as read just now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSnapshot {
    pub text: String,
    /// Selection as byte offsets into `text`; an empty range is the caret.
    pub selection: Option<Range<usize>>,
    pub whole_document: bool,
    pub scope: FieldScope,
}

impl FieldSnapshot {
    pub fn selected_text(&self) -> Option<&str> {
        self.selection
            .clone()
            .filter(|r| !r.is_empty())
            .and_then(|r| self.text.get(r))
    }

    fn caret(&self) -> usize {
        self.selection.as_ref().map_or(self.text.len(), |r| r.end)
    }
}

/// What a platform adapter provides for live corrections. Offsets are byte
/// offsets into the snapshot's text. Implementations restore the clipboard
/// themselves when they use it.
pub trait LiveField {
    /// Read the focused field again.
    fn read(&mut self) -> Result<FieldSnapshot, String>;
    /// Select `range` of `snapshot.text` (an empty range places the caret).
    fn select(&mut self, snapshot: &FieldSnapshot, range: Range<usize>) -> Result<(), String>;
    /// Read the current selection; `Ok(None)` when it cannot be read.
    fn read_selection(&mut self) -> Result<Option<String>, String>;
    /// Replace the selection with non-empty text.
    fn replace_selection(&mut self, text: &str) -> Result<(), String>;
    fn delete_selection(&mut self) -> Result<(), String>;
    /// Undo the application's last action once; only used to revert
    /// Striper's own change after it failed verification.
    fn undo_once(&mut self) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveError {
    Resolve(ResolveError),
    /// The operation needs Striper's dictation history (scratch that) or is
    /// not a text edit; use the ledger path.
    NotLive,
    /// The field kept changing while Striper was working.
    KeptChanging,
    /// The selected span was not the planned text; nothing was changed.
    Mismatch {
        expected: String,
        found: String,
    },
    /// The selection could not be read back, so nothing was changed.
    Unverifiable,
    /// The app altered the edit; `restored` says whether the original is back.
    PostVerifyFailed {
        restored: bool,
    },
    /// The edit would reach outside the user's selection.
    OutsideSelection,
    Platform(String),
}

impl std::fmt::Display for LiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LiveError::Resolve(ResolveError::NotFound(t)) => {
                write!(f, "I couldn't find \"{t}\" in the text near the cursor.")
            }
            LiveError::Resolve(e) => write!(f, "{e}"),
            LiveError::NotLive => write!(f, "That needs the recent dictation history."),
            LiveError::KeptChanging => write!(
                f,
                "The text kept changing while I worked, so I left it alone."
            ),
            LiveError::Mismatch { .. } => write!(
                f,
                "The text changed under me, so I left it alone."
            ),
            LiveError::Unverifiable => write!(
                f,
                "I can't check the text in this field, so I didn't change it."
            ),
            LiveError::PostVerifyFailed { restored: true } => {
                write!(f, "This app changed my edit, so I put the original text back.")
            }
            LiveError::PostVerifyFailed { restored: false } => {
                write!(f, "This app changed my edit and I couldn't verify the result.")
            }
            LiveError::OutsideSelection => write!(
                f,
                "I can only change the selected text in this app. Select the text to correct and try again."
            ),
            LiveError::Platform(e) => write!(f, "Editing failed: {e}"),
        }
    }
}

impl std::error::Error for LiveError {}

/// A bounded slice of the field around the caret: what the interpreter sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextWindow {
    /// Byte offset of the window in the snapshot text.
    pub offset: usize,
    pub text: String,
    /// True when the window is all of the field.
    pub complete: bool,
}

/// At most `max_chars` around the caret, mostly before it (corrections are
/// usually about what was just said), cut at word boundaries where possible.
pub fn context_window(snapshot: &FieldSnapshot, max_chars: usize) -> ContextWindow {
    let text = &snapshot.text;
    if text.chars().count() <= max_chars {
        return ContextWindow {
            offset: 0,
            text: text.clone(),
            complete: snapshot.whole_document,
        };
    }
    let (sel_start, sel_end) = snapshot
        .selection
        .as_ref()
        .map_or((text.len(), text.len()), |r| (r.start, r.end));
    let selected = text[sel_start..sel_end].chars().count();
    let budget = max_chars.saturating_sub(selected);
    let after_budget = budget / 5;
    let before_budget = budget - after_budget;
    let before_chars = text[..sel_start].chars().count();
    let after_chars = text[sel_end..].chars().count();
    // Give unused budget on one side to the other.
    let (take_before, take_after) = if after_chars < after_budget {
        (
            (before_budget + after_budget - after_chars).min(before_chars),
            after_chars,
        )
    } else if before_chars < before_budget {
        (
            before_chars,
            (after_budget + before_budget - before_chars).min(after_chars),
        )
    } else {
        (before_budget, after_budget)
    };
    let mut start = nth_char_from_end(&text[..sel_start], take_before);
    let mut end = sel_end + nth_char_offset(&text[sel_end..], take_after);
    // Avoid starting or ending mid-word.
    if start > 0 {
        if let Some(ws) = text[start..sel_start].find(char::is_whitespace) {
            start += ws;
            start += text[start..].chars().next().map_or(0, char::len_utf8);
        }
    }
    if end < text.len() {
        if let Some(ws) = text[sel_end..end].rfind(char::is_whitespace) {
            end = sel_end + ws;
        }
    }
    ContextWindow {
        offset: start,
        text: text[start..end].to_string(),
        complete: false,
    }
}

fn nth_char_from_end(s: &str, n: usize) -> usize {
    if n == 0 {
        return s.len();
    }
    s.char_indices().rev().nth(n - 1).map_or(0, |(i, _)| i)
}

fn nth_char_offset(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map_or(s.len(), |(i, _)| i)
}

/// A resolved change to the field: replace `range` of `base` with `replacement`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivePlan {
    pub base: FieldSnapshot,
    pub range: Range<usize>,
    pub expected: String,
    pub replacement: String,
    pub operation: String,
    /// The operation's own view of the change (e.g. "Thursday" → "Friday"
    /// rather than the minimal "Thurs" → "Fri"), for follow-ups and undo:
    /// where it starts in the base text, what it replaced, what it inserted.
    pub semantic_start: usize,
    pub semantic_before: String,
    pub semantic_after: String,
}

/// Whether an operation can be applied to live text.
pub fn is_live_operation(op: &EditOperation) -> bool {
    matches!(
        op,
        EditOperation::ReplaceText { .. }
            | EditOperation::ReplacePunctuation { .. }
            | EditOperation::InsertText { .. }
            | EditOperation::InsertParagraphBreaks { .. }
    )
}

/// Resolve one operation against a snapshot. The search area is the
/// selection when only the selection is readable, otherwise the context
/// window the interpreter saw (so it can't edit text it was never shown).
pub fn plan(
    snapshot: &FieldSnapshot,
    op: &EditOperation,
    window_chars: usize,
) -> Result<LivePlan, LiveError> {
    if !is_live_operation(op) {
        return Err(LiveError::NotLive);
    }
    let (offset, area) = match snapshot.scope {
        FieldScope::SelectionOnly => (0, snapshot.text.clone()),
        FieldScope::Field => {
            let w = context_window(snapshot, window_chars);
            (w.offset, w.text)
        }
    };
    let resolved = resolve_operation(&area, op).map_err(LiveError::Resolve)?;
    let prefix = common_prefix_len(&area, &resolved.new_text);
    let suffix = common_suffix_len(&area[prefix..], &resolved.new_text[prefix..]);
    let range = offset + prefix..offset + area.len() - suffix;
    let replacement = resolved.new_text[prefix..resolved.new_text.len() - suffix].to_string();
    Ok(LivePlan {
        expected: snapshot.text[range.clone()].to_string(),
        base: snapshot.clone(),
        range,
        replacement,
        operation: op.name().to_string(),
        semantic_start: offset + resolved.changed.start,
        semantic_before: resolved.replaced_text,
        semantic_after: resolved.inserted_text,
    })
}

fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n")
}

/// Grow an empty (pure insertion) range by one neighbouring grapheme so the
/// selection read-back always has something to verify.
fn verifiable_range(text: &str, range: Range<usize>, replacement: &str) -> (Range<usize>, String) {
    if !range.is_empty() {
        return (range, replacement.to_string());
    }
    if let Some(next) = text[range.end..].graphemes(true).next() {
        let r = range.start..range.end + next.len();
        return (r, format!("{replacement}{next}"));
    }
    if let Some(prev) = text[..range.start].graphemes(true).next_back() {
        let r = range.start - prev.len()..range.end;
        return (r, format!("{prev}{replacement}"));
    }
    (range, replacement.to_string())
}

/// Result of applying one plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveEdit {
    pub operation: String,
    /// The span that was replaced and what replaced it.
    pub before: String,
    pub after: String,
    /// Up to 40 characters before the change in the new text, to locate it
    /// again for "undo that".
    pub context_before: String,
    /// The field text after the edit, as verified.
    pub result: FieldSnapshot,
}

/// Apply a plan: verify the field still matches its base, select and verify
/// the span, replace it, verify the result, and put the caret back where the
/// user had it.
pub fn apply(
    field: &mut dyn LiveField,
    plan: &LivePlan,
    require_readback: bool,
) -> Result<LiveEdit, LiveError> {
    let p = LiveError::Platform;
    let fresh = field.read().map_err(p)?;
    if normalize(&fresh.text) != normalize(&plan.base.text) {
        return Err(LiveError::KeptChanging);
    }
    if plan.base.scope == FieldScope::SelectionOnly
        && plan
            .base
            .selection
            .as_ref()
            .is_some_and(|s| *s != (0..plan.base.text.len()))
    {
        return Err(LiveError::OutsideSelection);
    }
    let base = &fresh;
    let (range, replacement) = verifiable_range(&base.text, plan.range.clone(), &plan.replacement);
    let expected = base.text[range.clone()].to_string();
    field.select(base, range.clone()).map_err(p)?;
    match field.read_selection().map_err(p)? {
        Some(found) if normalize(&found) != normalize(&expected) => {
            let _ = restore_caret(field, base, base.caret());
            return Err(LiveError::Mismatch { expected, found });
        }
        Some(_) => {}
        None if require_readback => {
            let _ = restore_caret(field, base, base.caret());
            return Err(LiveError::Unverifiable);
        }
        None => {}
    }
    if replacement.is_empty() {
        field.delete_selection().map_err(p)?;
    } else {
        field.replace_selection(&replacement).map_err(p)?;
    }
    let desired = format!(
        "{}{}{}",
        &base.text[..range.start],
        replacement,
        &base.text[range.end..]
    );
    if base.scope == FieldScope::SelectionOnly {
        // Only the selection is readable: select what was just put in and
        // read it back.
        field
            .select(base, range.start..range.start + replacement.len())
            .map_err(p)?;
        let found = field.read_selection().map_err(p)?;
        if found.is_some_and(|f| normalize(&f) != normalize(&replacement)) {
            let restored = field.undo_once().is_ok();
            return Err(LiveError::PostVerifyFailed { restored });
        }
        return Ok(LiveEdit {
            operation: plan.operation.clone(),
            before: plan.semantic_before.clone(),
            after: plan.semantic_after.clone(),
            context_before: context_before(&desired, plan.semantic_start),
            result: FieldSnapshot {
                selection: Some(0..desired.len()),
                text: desired,
                whole_document: false,
                scope: FieldScope::SelectionOnly,
            },
        });
    }
    let after = field.read().map_err(p)?;
    let ok = if after.whole_document && base.whole_document {
        normalize(&after.text) == normalize(&desired)
    } else {
        // A window may shift; the changed neighbourhood must be present.
        let start = floor_char(&desired, range.start.saturating_sub(40));
        let end = ceil_char(
            &desired,
            (range.start + replacement.len() + 40).min(desired.len()),
        );
        normalize(&after.text).contains(&normalize(&desired[start..end]))
    };
    if !ok {
        let restored = field.undo_once().is_ok()
            && field
                .read()
                .is_ok_and(|b| normalize(&b.text) == normalize(&base.text));
        return Err(LiveError::PostVerifyFailed { restored });
    }
    // Keep the user's caret where it was, shifted by the change before it.
    let caret = base.caret();
    let new_caret = if caret >= range.end {
        caret + replacement.len() - (range.end - range.start)
    } else if caret <= range.start {
        caret
    } else {
        range.start + replacement.len()
    };
    let result = if after.whole_document {
        after
    } else {
        FieldSnapshot {
            text: desired.clone(),
            ..after
        }
    };
    if result.scope == FieldScope::Field {
        let _ = restore_caret(field, &result, new_caret.min(result.text.len()));
    }
    Ok(LiveEdit {
        operation: plan.operation.clone(),
        before: plan.semantic_before.clone(),
        after: plan.semantic_after.clone(),
        context_before: context_before(&desired, plan.semantic_start),
        result,
    })
}

fn context_before(text: &str, at: usize) -> String {
    let at = floor_char(text, at);
    let start = floor_char(text, at.saturating_sub(40));
    text[start..at].to_string()
}

fn restore_caret(
    field: &mut dyn LiveField,
    snapshot: &FieldSnapshot,
    caret: usize,
) -> Result<(), String> {
    let caret = floor_char(&snapshot.text, caret);
    field.select(snapshot, caret..caret)
}

fn floor_char(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Run text operations against the live field, in order. Each operation is
/// planned against a fresh read; when the field changed between planning and
/// applying, it is re-planned once against the new text.
pub fn run(
    field: &mut dyn LiveField,
    ops: &[EditOperation],
    window_chars: usize,
    require_readback: bool,
) -> Vec<Result<LiveEdit, LiveError>> {
    let mut results = Vec::new();
    for op in ops {
        let outcome = run_one(field, op, window_chars, require_readback);
        let failed = outcome.is_err();
        results.push(outcome);
        if failed {
            break;
        }
    }
    results
}

fn run_one(
    field: &mut dyn LiveField,
    op: &EditOperation,
    window_chars: usize,
    require_readback: bool,
) -> Result<LiveEdit, LiveError> {
    let mut last = LiveError::KeptChanging;
    for _attempt in 0..2 {
        let snapshot = field.read().map_err(LiveError::Platform)?;
        let plan = plan(&snapshot, op, window_chars)?;
        match apply(field, &plan, require_readback) {
            Err(LiveError::KeptChanging) => last = LiveError::KeptChanging,
            other => return other,
        }
    }
    Err(last)
}

/// "Undo that" for a live edit: find the inserted text where it was put
/// (identified by the text before it) and put the original back.
pub fn plan_undo(snapshot: &FieldSnapshot, edit: &LiveEdit) -> Result<LivePlan, LiveError> {
    let needle = format!("{}{}", edit.context_before, edit.after);
    let hits: Vec<usize> = snapshot
        .text
        .match_indices(&needle)
        .map(|(i, _)| i + edit.context_before.len())
        .collect();
    let start = match hits.as_slice() {
        [one] => *one,
        [] => {
            return Err(LiveError::Resolve(ResolveError::NotFound(
                edit.after.clone(),
            )))
        }
        many => {
            return Err(LiveError::Resolve(ResolveError::Ambiguous {
                text: edit.after.clone(),
                count: many.len(),
            }))
        }
    };
    let range = start..start + edit.after.len();
    Ok(LivePlan {
        base: snapshot.clone(),
        expected: edit.after.clone(),
        range,
        replacement: edit.before.clone(),
        operation: "undo_last_edit".into(),
        semantic_start: start,
        semantic_before: edit.after.clone(),
        semantic_after: edit.before.clone(),
    })
}

/// Undo a live edit with the same verification as any other edit.
pub fn undo(
    field: &mut dyn LiveField,
    edit: &LiveEdit,
    require_readback: bool,
) -> Result<LiveEdit, LiveError> {
    let mut last = LiveError::KeptChanging;
    for _attempt in 0..2 {
        let snapshot = field.read().map_err(LiveError::Platform)?;
        let plan = plan_undo(&snapshot, edit)?;
        match apply(field, &plan, require_readback) {
            Err(LiveError::KeptChanging) => last = LiveError::KeptChanging,
            other => return other,
        }
    }
    Err(last)
}

/// An in-memory field for tests and dry runs. `on_read` can simulate the
/// user typing while the interpreter thinks.
#[derive(Debug, Clone, Default)]
pub struct SimulatedLiveField {
    pub text: String,
    pub selection: Range<usize>,
    pub whole_document: bool,
    pub selection_only: bool,
    /// Text typed at the end on the given read (1-based), simulating a
    /// concurrent change.
    pub type_on_read: Option<(usize, String)>,
    pub unreadable_selection: bool,
    /// Collapse double spaces on replace, like some rich-text controls.
    pub collapses_spaces: bool,
    reads: usize,
    /// Where the selection-only snapshot starts in `text`.
    selection_base: Option<usize>,
    undo: Vec<(String, Range<usize>)>,
}

impl SimulatedLiveField {
    pub fn new(text: &str) -> Self {
        Self {
            text: text.into(),
            selection: text.len()..text.len(),
            whole_document: true,
            ..Default::default()
        }
    }

    pub fn reads(&self) -> usize {
        self.reads
    }
}

impl LiveField for SimulatedLiveField {
    fn read(&mut self) -> Result<FieldSnapshot, String> {
        self.reads += 1;
        if let Some((n, typed)) = &self.type_on_read {
            if *n == self.reads {
                self.text.push_str(typed);
            }
        }
        if self.selection_only {
            self.selection_base.get_or_insert(self.selection.start);
            let sel = self.text[self.selection.clone()].to_string();
            return Ok(FieldSnapshot {
                selection: Some(0..sel.len()),
                text: sel,
                whole_document: false,
                scope: FieldScope::SelectionOnly,
            });
        }
        Ok(FieldSnapshot {
            text: self.text.clone(),
            selection: Some(self.selection.clone()),
            whole_document: self.whole_document,
            scope: FieldScope::Field,
        })
    }

    fn select(&mut self, snapshot: &FieldSnapshot, range: Range<usize>) -> Result<(), String> {
        if self.selection_only {
            let base = *self.selection_base.get_or_insert(self.selection.start);
            self.selection = base + range.start..base + range.end;
            return Ok(());
        }
        if snapshot.text != self.text {
            return Err("field changed".into());
        }
        self.selection = range;
        Ok(())
    }

    fn read_selection(&mut self) -> Result<Option<String>, String> {
        if self.unreadable_selection {
            return Ok(None);
        }
        Ok(self.text.get(self.selection.clone()).map(str::to_string))
    }

    fn replace_selection(&mut self, text: &str) -> Result<(), String> {
        self.undo.push((self.text.clone(), self.selection.clone()));
        let text = if self.collapses_spaces {
            text.replace("  ", " ")
        } else {
            text.to_string()
        };
        let r = self.selection.clone();
        self.text.replace_range(r.clone(), &text);
        let caret = r.start + text.len();
        self.selection = caret..caret;
        Ok(())
    }

    fn delete_selection(&mut self) -> Result<(), String> {
        self.undo.push((self.text.clone(), self.selection.clone()));
        let r = self.selection.clone();
        self.text.replace_range(r.clone(), "");
        self.selection = r.start..r.start;
        Ok(())
    }

    fn undo_once(&mut self) -> Result<(), String> {
        let (text, sel) = self.undo.pop().ok_or("nothing to undo")?;
        self.text = text;
        self.selection = sel;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::InsertPosition;

    fn replace(e: &str, r: &str) -> EditOperation {
        EditOperation::ReplaceText {
            expected_text: e.into(),
            replacement_text: r.into(),
            occurrence: None,
        }
    }

    #[test]
    fn edits_text_the_user_typed_and_keeps_the_caret() {
        let mut f = SimulatedLiveField::new("Meet me on Thursday at noon. Bring the map.");
        f.selection = 28..28; // caret after "noon."
        let results = run(&mut f, &[replace("Thursday", "Friday")], 2400, true);
        let edit = results[0].as_ref().unwrap();
        assert_eq!(f.text, "Meet me on Friday at noon. Bring the map.");
        assert_eq!(
            (edit.before.as_str(), edit.after.as_str()),
            ("Thursday", "Friday")
        );
        // Caret shifted left by the two bytes removed before it.
        assert_eq!(f.selection, 26..26);
    }

    #[test]
    fn follow_up_and_undo_use_the_live_text() {
        let mut f = SimulatedLiveField::new("I'll see you on Thursday.");
        let first = run(&mut f, &[replace("Thursday", "Friday")], 2400, true)
            .remove(0)
            .unwrap();
        let second = run(&mut f, &[replace("Friday", "Saturday")], 2400, true)
            .remove(0)
            .unwrap();
        assert_eq!(f.text, "I'll see you on Saturday.");
        undo(&mut f, &second, true).unwrap();
        assert_eq!(f.text, "I'll see you on Friday.");
        undo(&mut f, &first, true).unwrap();
        assert_eq!(f.text, "I'll see you on Thursday.");
    }

    #[test]
    fn a_change_while_thinking_is_replanned_not_applied_stale() {
        // Read 1 plans, read 2 (pre-apply check) sees typing: re-plan on
        // read 3 against the new text and apply.
        let mut f = SimulatedLiveField::new("See you Thursday.");
        f.type_on_read = Some((2, " Also lunch?".into()));
        let r = run(&mut f, &[replace("Thursday", "Friday")], 2400, true);
        assert!(r[0].is_ok());
        assert_eq!(f.text, "See you Friday. Also lunch?");
    }

    #[test]
    fn a_target_removed_while_thinking_is_refused() {
        let mut f = SimulatedLiveField::new("See you Thursday.");
        let snap = f.read().unwrap();
        let p = plan(&snap, &replace("Thursday", "Friday"), 2400).unwrap();
        f.text = "See you soon.".into();
        f.selection = 13..13;
        assert_eq!(apply(&mut f, &p, true), Err(LiveError::KeptChanging));
        assert_eq!(f.text, "See you soon.");
        // And a fresh re-plan can't find it.
        let r = run(&mut f, &[replace("Thursday", "Friday")], 2400, true);
        assert!(matches!(
            r[0],
            Err(LiveError::Resolve(ResolveError::NotFound(_)))
        ));
    }

    #[test]
    fn unreadable_selection_refuses_unless_allowed() {
        let mut f = SimulatedLiveField::new("on Thursday");
        f.unreadable_selection = true;
        let r = run(&mut f, &[replace("Thursday", "Friday")], 2400, true);
        assert_eq!(r[0], Err(LiveError::Unverifiable));
        assert_eq!(f.text, "on Thursday");
        let r = run(&mut f, &[replace("Thursday", "Friday")], 2400, false);
        assert!(r[0].is_ok());
    }

    #[test]
    fn app_normalization_is_undone() {
        let mut f = SimulatedLiveField::new("End.Next");
        f.collapses_spaces = true;
        let op = EditOperation::ReplacePunctuation {
            expected_text: ".".into(),
            replacement_text: ".  ".into(),
            occurrence: None,
        };
        let r = run(&mut f, &[op], 2400, true);
        assert_eq!(r[0], Err(LiveError::PostVerifyFailed { restored: true }));
        assert_eq!(f.text, "End.Next");
    }

    #[test]
    fn paragraph_break_after_a_phrase_changes_only_whitespace() {
        let mut f =
            SimulatedLiveField::new("Thanks for the visit. See you tomorrow. Love to the kids.");
        let op = EditOperation::InsertParagraphBreaks {
            after: vec!["see you tomorrow".into()],
        };
        run(&mut f, &[op], 2400, true).remove(0).unwrap();
        assert_eq!(
            f.text,
            "Thanks for the visit. See you tomorrow.\n\nLove to the kids."
        );
    }

    #[test]
    fn splitting_into_paragraphs_keeps_every_word_and_windows_line_endings() {
        let mut f = SimulatedLiveField::new("Dear Ann,\r\nWe arrived. It rained. We left.");
        let op = EditOperation::InsertParagraphBreaks {
            after: vec!["We arrived.".into(), "It rained.".into()],
        };
        run(&mut f, &[op], 2400, true).remove(0).unwrap();
        assert_eq!(
            f.text,
            "Dear Ann,\r\nWe arrived.\r\n\r\nIt rained.\r\n\r\nWe left."
        );
    }

    #[test]
    fn existing_paragraph_breaks_are_left_alone() {
        let mut f = SimulatedLiveField::new("One.\n\nTwo.");
        let op = EditOperation::InsertParagraphBreaks {
            after: vec!["One.".into()],
        };
        let r = run(&mut f, &[op], 2400, true);
        assert_eq!(r[0], Err(LiveError::Resolve(ResolveError::NoChange)));
    }

    #[test]
    fn edits_outside_the_window_the_interpreter_saw_are_refused() {
        let early = "Thursday is far away. ";
        let filler = "word ".repeat(200);
        let mut f = SimulatedLiveField::new(&format!("{early}{filler}"));
        let r = run(&mut f, &[replace("Thursday", "Friday")], 300, true);
        assert!(matches!(
            r[0],
            Err(LiveError::Resolve(ResolveError::NotFound(_)))
        ));
    }

    #[test]
    fn context_window_is_mostly_before_the_caret_and_word_aligned() {
        let text = format!("{}CARET{}", "alpha ".repeat(100), " omega".repeat(100));
        let caret = text.find("CARET").unwrap() + 5;
        let snap = FieldSnapshot {
            selection: Some(caret..caret),
            text,
            whole_document: true,
            scope: FieldScope::Field,
        };
        let w = context_window(&snap, 100);
        assert!(!w.complete);
        assert!(w.text.contains("CARET"));
        assert!(w.text.starts_with("alpha"));
        let before = w.text.find("CARET").unwrap();
        assert!(before > w.text.len() - before);
        assert!(w.text.chars().count() <= 100);
    }

    #[test]
    fn selection_only_fields_confine_edits_to_the_selection() {
        let mut f = SimulatedLiveField::new("Keep this. Fix Thursday here. Keep that.");
        f.selection = 11..29;
        f.selection_only = true;
        run(&mut f, &[replace("Thursday", "Friday")], 2400, true)
            .remove(0)
            .unwrap();
        assert_eq!(f.text, "Keep this. Fix Friday here. Keep that.");
    }

    #[test]
    fn insertion_at_end_is_verified_with_a_neighbour() {
        let mut f = SimulatedLiveField::new("Hello");
        let op = EditOperation::InsertText {
            text: " world".into(),
            position: InsertPosition::End,
            anchor_text: None,
        };
        run(&mut f, &[op], 2400, true).remove(0).unwrap();
        assert_eq!(f.text, "Hello world");
    }

    #[test]
    fn ledger_only_operations_are_not_live() {
        let snap = SimulatedLiveField::new("x").read().unwrap();
        assert_eq!(
            plan(&snap, &EditOperation::DeleteRecentDictation, 100),
            Err(LiveError::NotLive)
        );
    }
}
