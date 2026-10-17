//! Deterministic text editing: resolving an [`EditOperation`] against the
//! recently inserted text, and executing the change in a target application
//! through a small, verifiable keystroke contract ([`TextTarget`]).
//!
//! Execution never sends a guessed number of Backspaces. It moves the caret
//! over text Striper itself inserted, selects exactly the span to change,
//! reads the selection back and compares it with what it expects (an
//! optimistic concurrency check, PLAN.md §9), and only then replaces it.
//! Any mismatch restores the caret and aborts.

use crate::command::{EditOperation, InsertPosition};
use crate::text::grapheme_len;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// The text to change is not in the editable region.
    NotFound(String),
    /// The text appears more than once and no occurrence was given.
    Ambiguous { text: String, count: usize },
    /// The occurrence asked for does not exist.
    OccurrenceOutOfRange { text: String, count: usize },
    /// The operation does not edit text (handled by the session instead).
    NotATextEdit,
    /// Nothing would change.
    NoChange,
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::NotFound(t) => write!(f, "I couldn't find \"{t}\" in the recent text."),
            ResolveError::Ambiguous { text, count } => {
                write!(
                    f,
                    "\"{text}\" appears {count} times; which one should I change?"
                )
            }
            ResolveError::OccurrenceOutOfRange { text, count } => {
                write!(f, "\"{text}\" only appears {count} time(s).")
            }
            ResolveError::NotATextEdit => write!(f, "That isn't a text edit."),
            ResolveError::NoChange => write!(f, "That wouldn't change anything."),
        }
    }
}

impl std::error::Error for ResolveError {}

/// Byte ranges of every occurrence of `needle` in `haystack`, matching
/// exactly first and falling back to a case-insensitive match (speech gives
/// no reliable case). Word-boundary matches are preferred so "on" does not
/// match inside "Thursday on".
pub fn find_occurrences(haystack: &str, needle: &str) -> Vec<std::ops::Range<usize>> {
    if needle.is_empty() {
        return Vec::new();
    }
    let exact: Vec<_> = haystack
        .match_indices(needle)
        .map(|(i, m)| i..i + m.len())
        .collect();
    let candidates = if exact.is_empty() {
        case_insensitive_matches(haystack, needle)
    } else {
        exact
    };
    let bounded: Vec<_> = candidates
        .iter()
        .filter(|r| at_word_boundaries(haystack, needle, r))
        .cloned()
        .collect();
    if bounded.is_empty() {
        candidates
    } else {
        bounded
    }
}

fn case_insensitive_matches(haystack: &str, needle: &str) -> Vec<std::ops::Range<usize>> {
    let needle_lower = needle.to_lowercase();
    let n = needle.chars().count();
    let starts: Vec<usize> = haystack.char_indices().map(|(i, _)| i).collect();
    let mut out = Vec::new();
    for (k, &start) in starts.iter().enumerate() {
        let end = starts.get(k + n).copied().unwrap_or(haystack.len());
        if k + n > starts.len() {
            break;
        }
        if haystack[start..end].to_lowercase() == needle_lower {
            out.push(start..end);
        }
    }
    out
}

fn at_word_boundaries(haystack: &str, needle: &str, r: &std::ops::Range<usize>) -> bool {
    let starts_word = needle.chars().next().is_some_and(|c| c.is_alphanumeric());
    let ends_word = needle.chars().last().is_some_and(|c| c.is_alphanumeric());
    let before_ok = !starts_word
        || haystack[..r.start]
            .chars()
            .last()
            .is_none_or(|c| !c.is_alphanumeric());
    let after_ok = !ends_word
        || haystack[r.end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric());
    before_ok && after_ok
}

fn pick(
    haystack: &str,
    needle: &str,
    occurrence: Option<usize>,
) -> Result<std::ops::Range<usize>, ResolveError> {
    let found = find_occurrences(haystack, needle);
    match (found.len(), occurrence) {
        (0, _) => Err(ResolveError::NotFound(needle.to_string())),
        (1, None | Some(1)) => Ok(found[0].clone()),
        (count, None) => Err(ResolveError::Ambiguous {
            text: needle.to_string(),
            count,
        }),
        (count, Some(n)) if n > count => Err(ResolveError::OccurrenceOutOfRange {
            text: needle.to_string(),
            count,
        }),
        (_, Some(n)) => Ok(found[n - 1].clone()),
    }
}

/// The result of resolving an operation: the region's new text and the
/// byte range (in the *new* text) that now holds the inserted/replaced text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEdit {
    pub new_text: String,
    pub changed: std::ops::Range<usize>,
    pub replaced_text: String,
    pub inserted_text: String,
}

/// Apply a text-editing operation to `region` (pure; no I/O).
pub fn resolve_operation(region: &str, op: &EditOperation) -> Result<ResolvedEdit, ResolveError> {
    let (range, replacement) = match op {
        EditOperation::ReplaceText {
            expected_text,
            replacement_text,
            occurrence,
        }
        | EditOperation::ReplacePunctuation {
            expected_text,
            replacement_text,
            occurrence,
        } => (
            pick(region, expected_text, *occurrence)?,
            replacement_text.clone(),
        ),
        EditOperation::InsertText {
            text,
            position,
            anchor_text,
        } => {
            let at = match position {
                InsertPosition::End => region.len(),
                InsertPosition::Start => 0,
                InsertPosition::Before | InsertPosition::After => {
                    let anchor = anchor_text.as_deref().unwrap_or_default();
                    let r = pick(region, anchor, None)?;
                    if *position == InsertPosition::Before {
                        r.start
                    } else {
                        r.end
                    }
                }
            };
            (at..at, text.clone())
        }
        _ => return Err(ResolveError::NotATextEdit),
    };
    let replaced_text = region[range.clone()].to_string();
    if replaced_text == replacement {
        return Err(ResolveError::NoChange);
    }
    let mut new_text = String::with_capacity(region.len() + replacement.len());
    new_text.push_str(&region[..range.start]);
    new_text.push_str(&replacement);
    new_text.push_str(&region[range.end..]);
    Ok(ResolvedEdit {
        changed: range.start..range.start + replacement.len(),
        new_text,
        replaced_text,
        inserted_text: replacement,
    })
}

/// A minimal keystroke plan turning `current` into `desired`, assuming the
/// caret sits at the end of `current`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditPlan {
    /// Caret steps left before selecting (the unchanged tail).
    pub suffix_steps: usize,
    /// Text that must be selected and read back before replacing.
    pub expected_selection: String,
    /// What replaces the selection (empty = delete it).
    pub replacement: String,
}

impl EditPlan {
    pub fn select_steps(&self) -> usize {
        grapheme_len(&self.expected_selection)
    }
}

/// Plan the edit by grapheme-level common prefix/suffix. The selection is
/// never empty: a pure insertion borrows one neighbouring grapheme so the
/// readback check always has something to verify.
pub fn plan_edit(current: &str, desired: &str) -> Option<EditPlan> {
    if current == desired || current.is_empty() {
        return None;
    }
    let a: Vec<&str> = current.graphemes(true).collect();
    let b: Vec<&str> = desired.graphemes(true).collect();
    let mut prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let max_suffix = a.len().min(b.len()) - prefix;
    let mut suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(max_suffix)
        .take_while(|(x, y)| x == y)
        .count();
    if a.len() - prefix - suffix == 0 {
        // Pure insertion: include a neighbouring grapheme in the selection.
        if suffix > 0 {
            suffix -= 1;
        } else {
            prefix -= 1;
        }
    }
    Some(EditPlan {
        suffix_steps: suffix,
        expected_selection: a[prefix..a.len() - suffix].concat(),
        replacement: b[prefix..b.len() - suffix].concat(),
    })
}

/// What a platform adapter must provide. Every method acts on the focused
/// text field of the target application; the adapter is responsible for
/// clipboard preservation inside `read_selection`/`replace_selection`.
pub trait TextTarget {
    fn move_left(&mut self, steps: usize) -> Result<(), String>;
    fn move_right(&mut self, steps: usize) -> Result<(), String>;
    /// Extend a selection leftwards from the caret by `steps`.
    fn select_left(&mut self, steps: usize) -> Result<(), String>;
    /// Read the current selection. `Ok(None)` when the platform cannot read
    /// it (the edit is then refused unless unverified edits are allowed).
    fn read_selection(&mut self) -> Result<Option<String>, String>;
    /// Replace the selection with non-empty text, leaving the caret after it.
    fn replace_selection(&mut self, text: &str) -> Result<(), String>;
    fn delete_selection(&mut self) -> Result<(), String>;
    /// Collapse a selection to its right edge (a plain Right press).
    fn collapse_right(&mut self) -> Result<(), String>;
    /// Undo the application's last action once (Ctrl+Z). Only used to revert
    /// Striper's own replacement immediately after it failed verification,
    /// and the result is read back before it is trusted.
    fn undo_once(&mut self) -> Result<(), String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecuteOptions {
    /// Refuse to edit when the selection cannot be read back.
    pub require_readback: bool,
    /// Re-select and read the replacement afterwards.
    pub verify_after: bool,
}

impl Default for ExecuteOptions {
    fn default() -> Self {
        Self {
            require_readback: true,
            verify_after: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecuteError {
    /// The text in the application is not what Striper inserted (the caret
    /// moved, the user typed, or the app changed it). Nothing was edited.
    Mismatch { expected: String, found: String },
    /// The selection could not be read, so the edit was not attempted.
    Unverifiable,
    /// The edit was applied but the result is not what was intended (for
    /// example the app collapsed double spaces). `restored` says whether the
    /// original text was put back.
    PostVerifyFailed { found: String, restored: bool },
    /// The platform adapter failed.
    Platform(String),
}

impl std::fmt::Display for ExecuteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecuteError::Mismatch { .. } => write!(
                f,
                "The text in the window no longer matches what I typed, so I left it alone."
            ),
            ExecuteError::Unverifiable => {
                write!(
                    f,
                    "I can't read the text in this window, so I didn't change it."
                )
            }
            ExecuteError::PostVerifyFailed { restored, .. } => {
                if *restored {
                    write!(
                        f,
                        "This app changed my edit, so I put the original text back."
                    )
                } else {
                    write!(
                        f,
                        "This app changed my edit and I couldn't verify the result."
                    )
                }
            }
            ExecuteError::Platform(e) => write!(f, "Editing failed: {e}"),
        }
    }
}

impl std::error::Error for ExecuteError {}

fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n")
}

fn texts_match(found: &str, expected: &str) -> bool {
    normalize_newlines(found) == normalize_newlines(expected)
}

/// Execute `plan` against `target`, verifying before and (optionally)
/// after. On success the caret is back at the end of the region.
pub fn execute_plan(
    target: &mut dyn TextTarget,
    plan: &EditPlan,
    opts: ExecuteOptions,
) -> Result<(), ExecuteError> {
    let p = ExecuteError::Platform;
    let restore_caret = |t: &mut dyn TextTarget| -> Result<(), ExecuteError> {
        t.collapse_right().map_err(p)?;
        t.move_right(plan.suffix_steps).map_err(p)
    };

    target.move_left(plan.suffix_steps).map_err(p)?;
    target.select_left(plan.select_steps()).map_err(p)?;
    match target.read_selection().map_err(p)? {
        Some(found) if !texts_match(&found, &plan.expected_selection) => {
            restore_caret(target)?;
            return Err(ExecuteError::Mismatch {
                expected: plan.expected_selection.clone(),
                found,
            });
        }
        Some(_) => {}
        None if opts.require_readback => {
            restore_caret(target)?;
            return Err(ExecuteError::Unverifiable);
        }
        None => {}
    }

    if plan.replacement.is_empty() {
        target.delete_selection().map_err(p)?;
    } else {
        target.replace_selection(&plan.replacement).map_err(p)?;
        if opts.verify_after {
            target
                .select_left(grapheme_len(&plan.replacement))
                .map_err(p)?;
            if let Some(found) = target.read_selection().map_err(p)? {
                if !texts_match(&found, &plan.replacement) {
                    // The app altered the text (e.g. collapsed spaces). Revert
                    // our own paste once and check that the original is back.
                    target.collapse_right().map_err(p)?;
                    let restored = target.undo_once().is_ok() && {
                        target.collapse_right().map_err(p)?;
                        target.select_left(plan.select_steps()).map_err(p)?;
                        let back = target.read_selection().map_err(p)?;
                        target.collapse_right().map_err(p)?;
                        back.is_some_and(|b| texts_match(&b, &plan.expected_selection))
                    };
                    target.move_right(plan.suffix_steps).map_err(p)?;
                    return Err(ExecuteError::PostVerifyFailed { found, restored });
                }
            }
            target.collapse_right().map_err(p)?;
        }
    }
    target.move_right(plan.suffix_steps).map_err(p)
}

/// An in-memory text field with caret and selection, implementing the same
/// contract as the platform adapters. Used by tests and by the dry-run
/// diagnostics; it models Windows edit-control semantics (Left/Right with a
/// selection collapse it, paste replaces the selection).
#[derive(Debug, Clone, Default)]
pub struct SimulatedField {
    pub graphemes: Vec<String>,
    pub caret: usize,
    pub anchor: Option<usize>,
    /// Simulate an app that cannot expose its selection.
    pub unreadable: bool,
    /// Simulate an app that collapses runs of spaces on paste.
    pub collapses_spaces: bool,
    /// Snapshots for undo: (text, caret, anchor) before each change.
    undo_stack: Vec<(Vec<String>, usize, Option<usize>)>,
}

impl SimulatedField {
    pub fn with_text(text: &str) -> Self {
        let graphemes: Vec<String> = text.graphemes(true).map(str::to_string).collect();
        let caret = graphemes.len();
        Self {
            graphemes,
            caret,
            ..Default::default()
        }
    }

    pub fn text(&self) -> String {
        self.graphemes.concat()
    }

    fn selection(&self) -> Option<(usize, usize)> {
        self.anchor
            .filter(|a| *a != self.caret)
            .map(|a| (a.min(self.caret), a.max(self.caret)))
    }

    fn snapshot(&mut self) {
        self.undo_stack
            .push((self.graphemes.clone(), self.caret, self.anchor));
    }

    fn insert(&mut self, text: &str) {
        self.snapshot();
        let text = if self.collapses_spaces {
            let mut s = String::new();
            for c in text.chars() {
                if !(c == ' ' && s.ends_with(' ')) {
                    s.push(c);
                }
            }
            s
        } else {
            text.to_string()
        };
        if let Some((s, e)) = self.selection() {
            self.graphemes.drain(s..e);
            self.caret = s;
        }
        self.anchor = None;
        let new: Vec<String> = text.graphemes(true).map(str::to_string).collect();
        let n = new.len();
        self.graphemes.splice(self.caret..self.caret, new);
        self.caret += n;
    }
}

impl TextTarget for SimulatedField {
    fn move_left(&mut self, steps: usize) -> Result<(), String> {
        if steps == 0 {
            return Ok(());
        }
        let mut remaining = steps;
        if let Some((s, _)) = self.selection() {
            self.caret = s;
            remaining -= 1;
        }
        self.anchor = None;
        self.caret = self.caret.saturating_sub(remaining);
        Ok(())
    }

    fn move_right(&mut self, steps: usize) -> Result<(), String> {
        if steps == 0 {
            return Ok(());
        }
        let mut remaining = steps;
        if let Some((_, e)) = self.selection() {
            self.caret = e;
            remaining -= 1;
        }
        self.anchor = None;
        self.caret = (self.caret + remaining).min(self.graphemes.len());
        Ok(())
    }

    fn select_left(&mut self, steps: usize) -> Result<(), String> {
        if self.anchor.is_none() {
            self.anchor = Some(self.caret);
        }
        self.caret = self.caret.saturating_sub(steps);
        Ok(())
    }

    fn read_selection(&mut self) -> Result<Option<String>, String> {
        if self.unreadable {
            return Ok(None);
        }
        Ok(Some(
            self.selection()
                .map(|(s, e)| self.graphemes[s..e].concat())
                .unwrap_or_default(),
        ))
    }

    fn replace_selection(&mut self, text: &str) -> Result<(), String> {
        self.insert(text);
        Ok(())
    }

    fn delete_selection(&mut self) -> Result<(), String> {
        self.snapshot();
        if let Some((s, e)) = self.selection() {
            self.graphemes.drain(s..e);
            self.caret = s;
        }
        self.anchor = None;
        Ok(())
    }

    fn collapse_right(&mut self) -> Result<(), String> {
        if let Some((_, e)) = self.selection() {
            self.caret = e;
        }
        self.anchor = None;
        Ok(())
    }

    fn undo_once(&mut self) -> Result<(), String> {
        let (graphemes, caret, anchor) = self.undo_stack.pop().ok_or("nothing to undo")?;
        self.graphemes = graphemes;
        self.caret = caret;
        self.anchor = anchor;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replace(expected: &str, replacement: &str) -> EditOperation {
        EditOperation::ReplaceText {
            expected_text: expected.into(),
            replacement_text: replacement.into(),
            occurrence: None,
        }
    }

    #[test]
    fn resolves_unique_replacement() {
        let r =
            resolve_operation("I'll see you on Thursday.", &replace("Thursday", "Friday")).unwrap();
        assert_eq!(r.new_text, "I'll see you on Friday.");
        assert_eq!(&r.new_text[r.changed.clone()], "Friday");
    }

    #[test]
    fn matching_is_case_insensitive_when_needed() {
        let r = resolve_operation("see you on Thursday", &replace("thursday", "Friday")).unwrap();
        assert_eq!(r.new_text, "see you on Friday");
    }

    #[test]
    fn prefers_whole_words() {
        let r = resolve_operation("Don is on time", &replace("on", "in")).unwrap();
        assert_eq!(r.new_text, "Don is in time");
    }

    #[test]
    fn ambiguous_and_missing_targets_are_refused() {
        assert_eq!(
            resolve_operation("the cat and the dog", &replace("the", "a")),
            Err(ResolveError::Ambiguous {
                text: "the".into(),
                count: 2
            })
        );
        assert_eq!(
            resolve_operation("hello", &replace("Thursday", "Friday")),
            Err(ResolveError::NotFound("Thursday".into()))
        );
        let second = EditOperation::ReplaceText {
            expected_text: "the".into(),
            replacement_text: "a".into(),
            occurrence: Some(2),
        };
        assert_eq!(
            resolve_operation("the cat and the dog", &second)
                .unwrap()
                .new_text,
            "the cat and a dog"
        );
    }

    #[test]
    fn resolves_insertions() {
        let end = EditOperation::InsertText {
            text: "  ".into(),
            position: InsertPosition::End,
            anchor_text: None,
        };
        assert_eq!(
            resolve_operation("Done.", &end).unwrap().new_text,
            "Done.  "
        );
        let after = EditOperation::InsertText {
            text: ",".into(),
            position: InsertPosition::After,
            anchor_text: Some("Friday".into()),
        };
        assert_eq!(
            resolve_operation("On Friday we go", &after)
                .unwrap()
                .new_text,
            "On Friday, we go"
        );
    }

    #[test]
    fn plan_selects_only_the_changed_span() {
        let plan = plan_edit("I'll see you on Thursday.", "I'll see you on Friday.").unwrap();
        assert_eq!(plan.suffix_steps, 4); // "day."
        assert_eq!(plan.expected_selection, "Thurs");
        assert_eq!(plan.replacement, "Fri");
    }

    #[test]
    fn plan_for_pure_insertion_borrows_a_grapheme() {
        let plan = plan_edit("Done.", "Done.  ").unwrap();
        assert_eq!(plan.expected_selection, ".");
        assert_eq!(plan.replacement, ".  ");
        assert_eq!(plan.suffix_steps, 0);
        let plan = plan_edit("ab", "aXb").unwrap();
        assert_eq!(
            (
                plan.expected_selection.as_str(),
                plan.replacement.as_str(),
                plan.suffix_steps
            ),
            ("b", "Xb", 0)
        );
    }

    #[test]
    fn plan_for_deletion() {
        let plan = plan_edit("Hello there.", "").unwrap();
        assert_eq!(plan.expected_selection, "Hello there.");
        assert_eq!(plan.replacement, "");
    }

    fn run(
        field_text: &str,
        current: &str,
        desired: &str,
    ) -> (Result<(), ExecuteError>, SimulatedField) {
        let mut field = SimulatedField::with_text(field_text);
        let plan = plan_edit(current, desired).unwrap();
        let r = execute_plan(&mut field, &plan, ExecuteOptions::default());
        (r, field)
    }

    #[test]
    fn executes_replacement_in_a_field_with_earlier_text() {
        let (r, field) = run(
            "Dear Bob, I'll see you on Thursday.",
            "I'll see you on Thursday.",
            "I'll see you on Friday.",
        );
        r.unwrap();
        assert_eq!(field.text(), "Dear Bob, I'll see you on Friday.");
        assert_eq!(field.caret, field.graphemes.len());
    }

    #[test]
    fn executes_deletion_and_insertion() {
        let (r, field) = run("Keep. Drop this.", "Drop this.", "");
        r.unwrap();
        assert_eq!(field.text(), "Keep. ");
        let (r, field) = run("Done.", "Done.", "Done.  ");
        r.unwrap();
        assert_eq!(field.text(), "Done.  ");
    }

    #[test]
    fn refuses_when_caret_moved() {
        let mut field = SimulatedField::with_text("I'll see you on Thursday.");
        field.caret = 4; // user clicked elsewhere
        let plan = plan_edit("I'll see you on Thursday.", "I'll see you on Friday.").unwrap();
        let r = execute_plan(&mut field, &plan, ExecuteOptions::default());
        assert!(matches!(r, Err(ExecuteError::Mismatch { .. })));
        assert_eq!(field.text(), "I'll see you on Thursday.");
    }

    #[test]
    fn refuses_when_text_was_changed_by_the_user() {
        let (r, field) = run(
            "I'll see you on Tuesday.",
            "I'll see you on Thursday.",
            "I'll see you on Friday.",
        );
        assert!(matches!(r, Err(ExecuteError::Mismatch { .. })));
        assert_eq!(field.text(), "I'll see you on Tuesday.");
        assert_eq!(field.caret, field.graphemes.len());
    }

    #[test]
    fn refuses_unreadable_fields_by_default() {
        let mut field = SimulatedField::with_text("on Thursday");
        field.unreadable = true;
        let plan = plan_edit("on Thursday", "on Friday").unwrap();
        assert_eq!(
            execute_plan(&mut field, &plan, ExecuteOptions::default()),
            Err(ExecuteError::Unverifiable)
        );
        assert_eq!(field.text(), "on Thursday");
        let opts = ExecuteOptions {
            require_readback: false,
            verify_after: true,
        };
        execute_plan(&mut field, &plan, opts).unwrap();
        assert_eq!(field.text(), "on Friday");
    }

    #[test]
    fn detects_app_normalization_and_restores() {
        let mut field = SimulatedField::with_text("End.");
        field.collapses_spaces = true;
        let plan = plan_edit("End.", "End.  ").unwrap();
        let r = execute_plan(&mut field, &plan, ExecuteOptions::default());
        assert!(matches!(
            r,
            Err(ExecuteError::PostVerifyFailed { restored: true, .. })
        ));
        assert_eq!(field.text(), "End.");
    }

    #[test]
    fn handles_unicode_graphemes() {
        let (r, field) = run("Café 👍🏽 Zoe", "Café 👍🏽 Zoe", "Café 👍🏽 Zoë");
        r.unwrap();
        assert_eq!(field.text(), "Café 👍🏽 Zoë");
    }
}
