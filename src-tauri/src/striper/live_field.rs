//! The real [`LiveField`]: reads the focused field and selects ranges
//! through the platform accessibility adapter (UI Automation on Windows),
//! and changes text through the same verified paste/keystroke path as
//! ledger edits (PLAN.md §9a).

use super::target::KeyboardTarget;
use striper_core::edit::TextTarget;
use striper_core::live::{FieldScope, FieldSnapshot, LiveField};
use striper_platform::live_text::{self, FocusedText, LiveTextError};

/// How much of a long document is read around the caret. The interpreter
/// sees a smaller window (`prompt::MAX_LIVE_CONTEXT_CHARS`); the margin lets
/// the window be found again after small changes.
pub const READ_CHARS: usize = 8_000;

pub fn to_snapshot(text: &FocusedText) -> FieldSnapshot {
    FieldSnapshot {
        text: text.text.clone(),
        selection: text.selection.clone(),
        whole_document: text.whole_document,
        scope: FieldScope::Field,
    }
}

/// Read the focused field, or say why it can't be used for live editing.
pub fn read() -> Result<FocusedText, LiveTextError> {
    let text = live_text::read_focused_text(READ_CHARS)?;
    if !text.can_select {
        return Err(LiveTextError::Unsupported(
            "the field can be read but not selected".into(),
        ));
    }
    Ok(text)
}

pub struct AccessibleField {
    keys: KeyboardTarget,
    /// The platform snapshot behind the last [`FieldSnapshot`] handed out,
    /// needed to select inside the same control.
    last: Option<FocusedText>,
}

impl AccessibleField {
    pub fn new(keys: KeyboardTarget) -> Self {
        Self { keys, last: None }
    }

    pub fn keys(&self) -> &KeyboardTarget {
        &self.keys
    }
}

impl LiveField for AccessibleField {
    fn read(&mut self) -> Result<FieldSnapshot, String> {
        let text = read().map_err(|e| e.to_string())?;
        if let Some(prev) = &self.last {
            if prev.control_id.is_some() && prev.control_id != text.control_id {
                return Err(LiveTextError::FocusChanged.to_string());
            }
        }
        let snapshot = to_snapshot(&text);
        self.last = Some(text);
        Ok(snapshot)
    }

    fn select(
        &mut self,
        snapshot: &FieldSnapshot,
        range: std::ops::Range<usize>,
    ) -> Result<(), String> {
        let base = self
            .last
            .as_ref()
            .filter(|t| t.text == snapshot.text)
            .cloned()
            .ok_or("the field changed since it was read")?;
        live_text::select_range(&base, range).map_err(|e| e.to_string())
    }

    fn read_selection(&mut self) -> Result<Option<String>, String> {
        // Accessibility read-back first: no clipboard involved.
        if let Ok(text) = live_text::read_focused_text(READ_CHARS) {
            if let Some(sel) = text.selection.clone() {
                return Ok(text.text.get(sel).map(str::to_string));
            }
        }
        self.keys.read_selection()
    }

    fn replace_selection(&mut self, text: &str) -> Result<(), String> {
        self.keys.replace_selection(text)
    }

    fn delete_selection(&mut self) -> Result<(), String> {
        self.keys.delete_selection()
    }

    fn undo_once(&mut self) -> Result<(), String> {
        self.keys.undo_once()
    }
}
