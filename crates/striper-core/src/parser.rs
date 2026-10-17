//! Instant, offline session controls for the correction lane.
//!
//! Corrections themselves are interpreted by the configured AI only; there is
//! deliberately no regex parser for edits. The few words recognised here
//! ("undo that", "done", "cancel") are controls that must work instantly and
//! without a network connection.

use crate::command::EditOperation;
use regex::Regex;
use std::sync::OnceLock;

/// The previous assistant edit, for follow-ups such as "actually, Saturday".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastEdit {
    pub replaced_text: String,
    pub inserted_text: String,
}

struct Controls {
    undo: Regex,
    finish: Regex,
    cancel: Regex,
}

fn controls() -> &'static Controls {
    static C: OnceLock<Controls> = OnceLock::new();
    C.get_or_init(|| {
        let re = |s: &str| Regex::new(&format!("(?i)^(?:{s})$")).expect("valid control pattern");
        Controls {
            undo: re(r"undo(?: that| it| this| the last (?:edit|change|correction))?|put it back|change it back"),
            finish: re(r"done|finished|i'?m done|that'?s all|that'?s it|all done|thank you|thanks|good|perfect|that'?s good|that'?s fine"),
            cancel: re(r"cancel|never ?mind|forget it|stop|quit"),
        }
    })
}

/// The control the whole instruction asks for, if it is only a control.
/// Leading "correct that" and surrounding punctuation are ignored.
pub fn local_control(instruction: &str) -> Option<EditOperation> {
    let mut text = instruction.trim();
    if let Some(rest) = crate::dictation::correction_prefix(text) {
        text = rest;
    }
    let text = text.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'');
    let c = controls();
    if c.undo.is_match(text) {
        Some(EditOperation::UndoLastEdit)
    } else if c.finish.is_match(text) {
        Some(EditOperation::Finish)
    } else if c.cancel.is_match(text) {
        Some(EditOperation::Cancel)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_are_recognised_locally() {
        assert_eq!(
            local_control("Undo that."),
            Some(EditOperation::UndoLastEdit)
        );
        assert_eq!(
            local_control("Correct that, undo it"),
            Some(EditOperation::UndoLastEdit)
        );
        assert_eq!(local_control("Done!"), Some(EditOperation::Finish));
        assert_eq!(local_control("never mind"), Some(EditOperation::Cancel));
    }

    #[test]
    fn edits_are_left_to_the_ai() {
        for said in [
            "change Thursday to Friday",
            "I meant Striper, S-T-R-I-P-E-R, remember that",
            "start a new paragraph after see you tomorrow",
            "undo the paragraph about the dog and make it warmer",
            "scratch that",
        ] {
            assert_eq!(local_control(said), None, "{said}");
        }
    }
}
