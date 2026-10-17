//! Live text of the focused field through accessibility APIs (PLAN.md §9a).
//!
//! The focused application's text is the source of truth for corrections.
//! This module only *reads* the field and *selects* a range in it; changing
//! the text stays on the verified paste/keystroke path, which reads the
//! result back before trusting it. Pixels are never read.
//!
//! Offsets are UTF-8 byte offsets into [`FocusedText::text`].

use std::ops::Range;

/// A snapshot of the focused text field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FocusedText {
    /// The field's text, or a bounded window of it around the caret when the
    /// document is larger than requested (see `whole_document`).
    pub text: String,
    /// Selection as byte offsets into `text`; an empty range is the caret.
    /// `None` when the API does not expose it.
    pub selection: Option<Range<usize>>,
    /// True when `text` is the entire field, false when it is a window.
    pub whole_document: bool,
    /// Stable identity of the control (UIA runtime id, AT-SPI path), used to
    /// make sure a later selection lands in the same field.
    pub control_id: Option<String>,
    /// Whether [`select_range`] is supported for this field.
    pub can_select: bool,
    /// Which API produced the snapshot, for diagnostics ("uia-text",
    /// "uia-value", "atspi").
    pub source: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveTextError {
    /// Nothing focused, or the focus is not a text field.
    NoTextField,
    /// A password or otherwise protected field: never read or edited.
    Protected,
    /// The field exists but does not expose its text through any supported
    /// API on this platform.
    Unsupported(String),
    /// The field changed identity since it was read.
    FocusChanged,
    /// The platform call failed.
    Failed(String),
}

impl std::fmt::Display for LiveTextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LiveTextError::NoTextField => write!(f, "No text field is focused."),
            LiveTextError::Protected => write!(
                f,
                "This is a protected field; Striper won't read or edit it."
            ),
            LiveTextError::Unsupported(why) => {
                write!(f, "This app doesn't let Striper read the field ({why}).")
            }
            LiveTextError::FocusChanged => write!(f, "The focused field changed."),
            LiveTextError::Failed(e) => write!(f, "Reading the field failed: {e}"),
        }
    }
}

impl std::error::Error for LiveTextError {}

/// Read the focused text field. `max_chars` bounds how much text is
/// returned; a larger document yields a window around the caret with
/// `whole_document == false`.
pub fn read_focused_text(max_chars: usize) -> Result<FocusedText, LiveTextError> {
    imp::read_focused_text(max_chars)
}

/// Select `range` (byte offsets into `snapshot.text`) in the focused field.
/// Fails with [`LiveTextError::FocusChanged`] when the focused control is no
/// longer `snapshot.control_id`. Callers must read the selection back and
/// compare it with the expected text before replacing it: character units
/// differ between providers, so a selection is never trusted unverified.
pub fn select_range(snapshot: &FocusedText, range: Range<usize>) -> Result<(), LiveTextError> {
    if range.start > range.end
        || range.end > snapshot.text.len()
        || !snapshot.text.is_char_boundary(range.start)
        || !snapshot.text.is_char_boundary(range.end)
    {
        return Err(LiveTextError::Failed("range outside the field text".into()));
    }
    imp::select_range(snapshot, range)
}

#[cfg(windows)]
#[path = "live_text_windows.rs"]
mod imp;

#[cfg(not(windows))]
mod imp {
    //! AT-SPI support on Linux is planned (PLAN.md §9a); until then callers
    //! fall back to the selection and the dictation ledger.
    use super::*;

    pub fn read_focused_text(_max_chars: usize) -> Result<FocusedText, LiveTextError> {
        Err(LiveTextError::Unsupported(
            "accessibility text reading is not implemented on this platform yet".into(),
        ))
    }

    pub fn select_range(
        _snapshot: &FocusedText,
        _range: Range<usize>,
    ) -> Result<(), LiveTextError> {
        Err(LiveTextError::Unsupported(
            "accessibility selection is not implemented on this platform yet".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_range_rejects_out_of_bounds_and_split_characters() {
        let snapshot = FocusedText {
            text: "café".into(),
            ..Default::default()
        };
        assert!(matches!(
            select_range(&snapshot, 0..99),
            Err(LiveTextError::Failed(_))
        ));
        // 'é' is two bytes; offset 4 splits it.
        assert!(matches!(
            select_range(&snapshot, 0..4),
            Err(LiveTextError::Failed(_))
        ));
    }

    #[test]
    fn reading_never_panics() {
        let _ = read_focused_text(4000);
    }
}
