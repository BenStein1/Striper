//! The bounded request sent to a command-interpreting LLM (PLAN.md §7).
//!
//! Only the spoken instruction, the recent editable text (capped) and the
//! previous edits of this session are sent: never the window contents,
//! clipboard, or anything else from the desktop.

use crate::command::MAX_OPERATIONS;
use crate::parser::LastEdit;
use serde::Serialize;

/// Maximum characters of recent text included in a request.
pub const MAX_CONTEXT_CHARS: usize = 1200;

pub const SYSTEM_PROMPT: &str = r#"You turn a spoken correction instruction into edit operations for text the user just dictated.
You never see or control the computer. You only return JSON matching the schema; the application validates and performs the edit itself.

Input JSON fields:
- "instruction": what the user said (speech-recognized, may contain recognition errors).
- "recent_text": the text the user recently dictated. Only this text can be edited.
- "last_dictation": the most recent dictation (the end of recent_text).
- "previous_edits": edits already made in this session, oldest first, as {"replaced": ..., "inserted": ...}.

Return {"operations": [...], "confidence": 0..1}. Allowed operations:
- {"op":"replace_text","expected_text":E,"replacement_text":R,"occurrence":N|null}: E must be copied exactly from recent_text. Use "" for R to delete. Set occurrence (1-based) only when E appears more than once and the user said which.
- {"op":"replace_punctuation", same fields}: only punctuation/space changes.
- {"op":"insert_text","text":T,"position":"before"|"after"|"end"|"start","anchor_text":A|null}: A copied exactly from recent_text for before/after.
- {"op":"delete_recent_dictation"}: "scratch that" — remove the last dictation.
- {"op":"undo_last_edit"}: reverse the previous edit.
- {"op":"remember_spelling","preferred_text":P,"aliases":[misrecognized forms],"context_words":[single words that signal this meaning],"always":false}: only when the user explicitly asks to remember/learn a spelling. Set always=true only if they say it should always be replaced.
- {"op":"forget_spelling","text":P|null}: the user wants a saved spelling forgotten.
- {"op":"clarify","question":Q}: the target is ambiguous or not in recent_text. Ask a short question instead of guessing.
- {"op":"cancel"} / {"op":"finish"}: the user is cancelling or done.

Rules:
- "that"/"it" in follow-ups like "actually make it Saturday" refers to the last previous edit's inserted text.
- Spelled letters ("S-T-E-P-H-E-N") give the exact spelling; keep the user's capitalization of names.
- Never invent text the user did not ask for. Do not rewrite style or grammar unless asked.
- Prefer one operation. At most __MAX_OPS__ operations.
- If unsure, use clarify."#;

pub fn system_prompt() -> String {
    SYSTEM_PROMPT.replace("__MAX_OPS__", &MAX_OPERATIONS.to_string())
}

#[derive(Debug, Serialize)]
struct PreviousEdit<'a> {
    replaced: &'a str,
    inserted: &'a str,
}

#[derive(Debug, Serialize)]
struct Request<'a> {
    instruction: &'a str,
    recent_text: String,
    last_dictation: String,
    previous_edits: Vec<PreviousEdit<'a>>,
}

/// Keep the end of `text` (the most recent words) within `max` characters.
fn tail_chars(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    text.chars().skip(count - max).collect()
}

/// Build the user message for the interpreter.
pub fn build_user_message(
    instruction: &str,
    recent_text: &str,
    last_dictation: &str,
    previous_edits: &[LastEdit],
) -> String {
    let request = Request {
        instruction,
        recent_text: tail_chars(recent_text, MAX_CONTEXT_CHARS),
        last_dictation: tail_chars(last_dictation, MAX_CONTEXT_CHARS),
        previous_edits: previous_edits
            .iter()
            .rev()
            .take(5)
            .rev()
            .map(|e| PreviousEdit {
                replaced: &e.replaced_text,
                inserted: &e.inserted_text,
            })
            .collect(),
    };
    serde_json::to_string(&request).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_is_bounded_to_recent_text() {
        let long = format!("{}END", "x".repeat(5000));
        let msg = build_user_message("change END to FIN", &long, "END", &[]);
        let v: serde_json::Value = serde_json::from_str(&msg).unwrap();
        let recent = v["recent_text"].as_str().unwrap();
        assert_eq!(recent.chars().count(), MAX_CONTEXT_CHARS);
        assert!(recent.ends_with("END"));
    }

    #[test]
    fn includes_previous_edits() {
        let edits = vec![LastEdit {
            replaced_text: "Thursday".into(),
            inserted_text: "Friday".into(),
        }];
        let msg = build_user_message("make it Saturday", "on Friday", "on Friday", &edits);
        assert!(msg.contains("\"inserted\":\"Friday\""));
        assert!(system_prompt().contains("At most 4 operations"));
    }
}
