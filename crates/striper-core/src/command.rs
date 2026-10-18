//! The closed set of edit operations a correction instruction may produce,
//! and strict validation of operations proposed by an LLM (PLAN.md §7).
//!
//! The LLM never touches the desktop: it returns JSON naming one of these
//! operations, which is parsed with unknown fields rejected, bounded, and
//! then checked against the current text before anything is executed.

use serde::{Deserialize, Serialize};

/// Upper bound on any text an operation may introduce.
// Permit a rewrite of the 1,200-character prompt context with room to expand.
pub const MAX_OPERATION_TEXT: usize = 2400;
/// Upper bound on operations in one instruction.
pub const MAX_OPERATIONS: usize = 4;
/// Upper bound on paragraph breaks one operation may insert.
pub const MAX_PARAGRAPH_BREAKS: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsertPosition {
    /// Immediately before `anchor_text`.
    Before,
    /// Immediately after `anchor_text`.
    After,
    /// At the end of the editable text.
    End,
    /// At the start of the editable text.
    Start,
}

/// One validated editing intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum EditOperation {
    /// Replace an exact span of the recent text. An empty replacement
    /// deletes it. `occurrence` (1-based) disambiguates repeated spans.
    ReplaceText {
        expected_text: String,
        replacement_text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        occurrence: Option<usize>,
    },
    /// Same as `replace_text`, but only punctuation and whitespace may change;
    /// the words in the span must stay exactly the same.
    ReplacePunctuation {
        expected_text: String,
        replacement_text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        occurrence: Option<usize>,
    },
    InsertText {
        text: String,
        position: InsertPosition,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        anchor_text: Option<String>,
    },
    /// Start a new paragraph after each anchor (copied exactly from the
    /// text). Only whitespace changes, so the wording can never be altered:
    /// "start a new paragraph after 'see you tomorrow'", "split this into
    /// paragraphs".
    InsertParagraphBreaks {
        after: Vec<String>,
    },
    /// "Scratch that": remove the most recent dictation.
    DeleteRecentDictation,
    /// Reverse the last assistant edit.
    UndoLastEdit,
    /// Save a spelling preference. Only persisted when the user's own words
    /// asked for it (see `instruction_requests_memory`).
    RememberSpelling {
        preferred_text: String,
        #[serde(default)]
        aliases: Vec<String>,
        #[serde(default)]
        context_words: Vec<String>,
        #[serde(default)]
        always: bool,
    },
    /// Disable a saved spelling (by preferred text or alias); `None` means
    /// the most recently learned one.
    ForgetSpelling {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// Ask the user a question instead of guessing.
    Clarify {
        question: String,
    },
    Cancel,
    Finish,
}

impl EditOperation {
    pub fn name(&self) -> &'static str {
        match self {
            EditOperation::ReplaceText { .. } => "replace_text",
            EditOperation::ReplacePunctuation { .. } => "replace_punctuation",
            EditOperation::InsertText { .. } => "insert_text",
            EditOperation::InsertParagraphBreaks { .. } => "insert_paragraph_breaks",
            EditOperation::DeleteRecentDictation => "delete_recent_dictation",
            EditOperation::UndoLastEdit => "undo_last_edit",
            EditOperation::RememberSpelling { .. } => "remember_spelling",
            EditOperation::ForgetSpelling { .. } => "forget_spelling",
            EditOperation::Clarify { .. } => "clarify",
            EditOperation::Cancel => "cancel",
            EditOperation::Finish => "finish",
        }
    }

    /// Whether this operation changes text in the target application.
    pub fn edits_text(&self) -> bool {
        matches!(
            self,
            EditOperation::ReplaceText { .. }
                | EditOperation::ReplacePunctuation { .. }
                | EditOperation::InsertText { .. }
                | EditOperation::InsertParagraphBreaks { .. }
                | EditOperation::DeleteRecentDictation
                | EditOperation::UndoLastEdit
        )
    }
}

/// The JSON envelope an LLM must return.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommandResponse {
    pub operations: Vec<EditOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CommandError {
    InvalidJson(String),
    NoOperations,
    TooManyOperations(usize),
    TextTooLong(&'static str),
    ControlCharacters(&'static str),
    EmptyField(&'static str),
    NotPunctuation,
    MissingAnchor,
    InvalidOccurrence,
    LowConfidence(f64),
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandError::InvalidJson(e) => write!(f, "response was not a valid command: {e}"),
            CommandError::NoOperations => write!(f, "response contained no operations"),
            CommandError::TooManyOperations(n) => write!(f, "response contained {n} operations"),
            CommandError::TextTooLong(field) => write!(f, "{field} is too long"),
            CommandError::ControlCharacters(field) => {
                write!(f, "{field} contains control characters")
            }
            CommandError::EmptyField(field) => write!(f, "{field} is empty"),
            CommandError::NotPunctuation => {
                write!(
                    f,
                    "replace_punctuation may only change punctuation or spaces"
                )
            }
            CommandError::MissingAnchor => write!(f, "insert before/after needs anchor_text"),
            CommandError::InvalidOccurrence => write!(f, "occurrence must be 1 or more"),
            CommandError::LowConfidence(c) => write!(f, "confidence {c} is too low to act on"),
        }
    }
}

impl std::error::Error for CommandError {}

/// Operations proposed with confidence below this become a clarification.
pub const MIN_CONFIDENCE: f64 = 0.5;

/// JSON Schema handed to providers that support structured output.
pub fn response_json_schema() -> serde_json::Value {
    let text = serde_json::json!({ "type": "string", "maxLength": MAX_OPERATION_TEXT });
    let nullable_text =
        serde_json::json!({ "type": ["string", "null"], "maxLength": MAX_OPERATION_TEXT });
    let nullable_int = serde_json::json!({ "type": ["integer", "null"], "minimum": 1 });
    let op = |name: &str, props: serde_json::Value, required: &[&str]| {
        let mut properties = serde_json::Map::new();
        properties.insert(
            "op".into(),
            serde_json::json!({ "type": "string", "enum": [name] }),
        );
        if let serde_json::Value::Object(p) = props {
            properties.extend(p);
        }
        let mut req = vec!["op".to_string()];
        req.extend(required.iter().map(|s| s.to_string()));
        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": req,
            "additionalProperties": false
        })
    };
    serde_json::json!({
        "type": "object",
        "properties": {
            "operations": {
                "type": "array",
                "maxItems": MAX_OPERATIONS,
                "items": { "anyOf": [
                    op("replace_text", serde_json::json!({"expected_text": text, "replacement_text": text, "occurrence": nullable_int}), &["expected_text", "replacement_text", "occurrence"]),
                    op("replace_punctuation", serde_json::json!({"expected_text": text, "replacement_text": text, "occurrence": nullable_int}), &["expected_text", "replacement_text", "occurrence"]),
                    op("insert_text", serde_json::json!({"text": text, "position": {"type": "string", "enum": ["before", "after", "end", "start"]}, "anchor_text": nullable_text}), &["text", "position", "anchor_text"]),
                    op("insert_paragraph_breaks", serde_json::json!({"after": {"type": "array", "items": text, "minItems": 1, "maxItems": MAX_PARAGRAPH_BREAKS}}), &["after"]),
                    op("delete_recent_dictation", serde_json::json!({}), &[]),
                    op("undo_last_edit", serde_json::json!({}), &[]),
                    op("remember_spelling", serde_json::json!({"preferred_text": text, "aliases": {"type": "array", "items": text, "maxItems": 8}, "context_words": {"type": "array", "items": text, "maxItems": 12}, "always": {"type": "boolean"}}), &["preferred_text", "aliases", "context_words", "always"]),
                    op("forget_spelling", serde_json::json!({"text": nullable_text}), &["text"]),
                    op("clarify", serde_json::json!({"question": text}), &["question"]),
                    op("cancel", serde_json::json!({}), &[]),
                    op("finish", serde_json::json!({}), &[])
                ]}
            },
            "confidence": { "type": "number", "minimum": 0, "maximum": 1 }
        },
        "required": ["operations", "confidence"],
        "additionalProperties": false
    })
}

fn strip_code_fence(s: &str) -> &str {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```") {
        let rest = rest.strip_prefix("json").unwrap_or(rest);
        if let Some(body) = rest.strip_suffix("```") {
            return body.trim();
        }
    }
    t
}

/// Parse and validate an LLM response. Anything outside the contract is an
/// error; the caller must not fall back to interpreting free text.
pub fn parse_command_response(raw: &str) -> Result<CommandResponse, CommandError> {
    let body = strip_code_fence(raw);
    // Some providers emit `null` for optional fields; normalise those away
    // before the strict parse so `deny_unknown_fields` still catches extras.
    let mut value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| CommandError::InvalidJson(e.to_string()))?;
    if let Some(ops) = value.get_mut("operations").and_then(|o| o.as_array_mut()) {
        for op in ops {
            if let Some(obj) = op.as_object_mut() {
                obj.retain(|_, v| !v.is_null());
            }
        }
    }
    if value.get("confidence").is_some_and(|c| c.is_null()) {
        value.as_object_mut().map(|o| o.remove("confidence"));
    }
    let response: CommandResponse =
        serde_json::from_value(value).map_err(|e| CommandError::InvalidJson(e.to_string()))?;
    validate_response(&response)?;
    Ok(response)
}

pub fn validate_response(response: &CommandResponse) -> Result<(), CommandError> {
    if response.operations.is_empty() {
        return Err(CommandError::NoOperations);
    }
    if response.operations.len() > MAX_OPERATIONS {
        return Err(CommandError::TooManyOperations(response.operations.len()));
    }
    if let Some(c) = response.confidence {
        if !(MIN_CONFIDENCE..=1.0).contains(&c) {
            return Err(CommandError::LowConfidence(c));
        }
    }
    for op in &response.operations {
        validate_operation(op)?;
    }
    Ok(())
}

fn check_text(field: &'static str, s: &str, allow_empty: bool) -> Result<(), CommandError> {
    if !allow_empty && s.is_empty() {
        return Err(CommandError::EmptyField(field));
    }
    if s.chars().count() > MAX_OPERATION_TEXT {
        return Err(CommandError::TextTooLong(field));
    }
    // Newlines are legitimate dictation; every other control character
    // (escape sequences, tabs that move focus, NUL) is refused.
    if s.chars().any(|c| c.is_control() && c != '\n') {
        return Err(CommandError::ControlCharacters(field));
    }
    Ok(())
}

/// Whether `replacement` differs from `expected` only in punctuation and
/// spacing. Surrounding words may be included to locate the change
/// ("park.Then" -> "park. Then"), but must come through unchanged.
fn only_punctuation_differs(expected: &str, replacement: &str) -> bool {
    let words = |s: &str| {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
    };
    words(expected) == words(replacement)
}

pub fn validate_operation(op: &EditOperation) -> Result<(), CommandError> {
    match op {
        EditOperation::ReplaceText {
            expected_text,
            replacement_text,
            occurrence,
        } => {
            check_text("expected_text", expected_text, false)?;
            check_text("replacement_text", replacement_text, true)?;
            if *occurrence == Some(0) {
                return Err(CommandError::InvalidOccurrence);
            }
        }
        EditOperation::ReplacePunctuation {
            expected_text,
            replacement_text,
            occurrence,
        } => {
            check_text("expected_text", expected_text, false)?;
            check_text("replacement_text", replacement_text, true)?;
            if !only_punctuation_differs(expected_text, replacement_text) {
                return Err(CommandError::NotPunctuation);
            }
            if *occurrence == Some(0) {
                return Err(CommandError::InvalidOccurrence);
            }
        }
        EditOperation::InsertText {
            text,
            position,
            anchor_text,
        } => {
            check_text("text", text, false)?;
            match position {
                InsertPosition::Before | InsertPosition::After => match anchor_text {
                    Some(a) => check_text("anchor_text", a, false)?,
                    None => return Err(CommandError::MissingAnchor),
                },
                InsertPosition::End | InsertPosition::Start => {}
            }
        }
        EditOperation::InsertParagraphBreaks { after } => {
            if after.is_empty() {
                return Err(CommandError::EmptyField("after"));
            }
            if after.len() > MAX_PARAGRAPH_BREAKS {
                return Err(CommandError::TooManyOperations(after.len()));
            }
            for anchor in after {
                check_text("after", anchor, false)?;
                if anchor.trim().is_empty() {
                    return Err(CommandError::EmptyField("after"));
                }
            }
        }
        EditOperation::RememberSpelling {
            preferred_text,
            aliases,
            context_words,
            ..
        } => {
            check_text("preferred_text", preferred_text.trim(), false)?;
            if preferred_text.chars().count() > 80 {
                return Err(CommandError::TextTooLong("preferred_text"));
            }
            if aliases.len() > 8 || context_words.len() > 12 {
                return Err(CommandError::TooManyOperations(
                    aliases.len().max(context_words.len()),
                ));
            }
            for a in aliases {
                check_text("aliases", a, false)?;
            }
            for w in context_words {
                check_text("context_words", w, false)?;
            }
        }
        EditOperation::ForgetSpelling { text } => {
            if let Some(t) = text {
                check_text("text", t, false)?;
            }
        }
        EditOperation::Clarify { question } => check_text("question", question, false)?,
        EditOperation::DeleteRecentDictation
        | EditOperation::UndoLastEdit
        | EditOperation::Cancel
        | EditOperation::Finish => {}
    }
    Ok(())
}

/// Whether the user's own instruction asked for something to be remembered.
/// This, not the model's output, is what authorises persisting a rule.
pub fn instruction_requests_memory(instruction: &str) -> bool {
    let lower = instruction.to_lowercase();
    let negated = [
        "don't remember",
        "do not remember",
        "dont remember",
        "don't learn",
        "do not learn",
        "forget",
    ];
    if negated.iter().any(|n| lower.contains(n)) {
        return false;
    }
    [
        "remember",
        "learn that",
        "learn this",
        "learn it",
        "always spell",
        "always write",
        "from now on",
        "save that spelling",
        "save this spelling",
    ]
    .iter()
    .any(|k| lower.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Strict structured output writes keys in schema order. If "op" is not
    /// first, the model must commit to a field such as `expected_text` before
    /// naming the operation, and in practice only operations whose other
    /// fields sort after "op" (forget_spelling, clarify, ...) stay reachable.
    #[test]
    fn schema_names_the_operation_first() {
        let schema = response_json_schema();
        let variants = schema["properties"]["operations"]["items"]["anyOf"]
            .as_array()
            .unwrap();
        for variant in variants {
            let props = variant["properties"].as_object().unwrap();
            assert_eq!(
                props.keys().next().map(String::as_str),
                Some("op"),
                "{variant}"
            );
            assert_eq!(variant["required"][0], "op");
        }
        let top: Vec<_> = schema["properties"].as_object().unwrap().keys().collect();
        assert_eq!(top, ["operations", "confidence"]);
    }

    #[test]
    fn parses_a_valid_replace() {
        let r = parse_command_response(
            r#"{"operations":[{"op":"replace_text","expected_text":"Thursday","replacement_text":"Friday"}],"confidence":0.96}"#,
        )
        .unwrap();
        assert_eq!(
            r.operations[0],
            EditOperation::ReplaceText {
                expected_text: "Thursday".into(),
                replacement_text: "Friday".into(),
                occurrence: None
            }
        );
    }

    #[test]
    fn accepts_nulls_and_code_fences() {
        let r = parse_command_response(
            "```json\n{\"operations\":[{\"op\":\"replace_text\",\"expected_text\":\"a\",\"replacement_text\":\"b\",\"occurrence\":null}],\"confidence\":null}\n```",
        )
        .unwrap();
        assert_eq!(r.operations.len(), 1);
    }

    #[test]
    fn rejects_unknown_operations() {
        let e =
            parse_command_response(r#"{"operations":[{"op":"run_shell","command":"rm -rf /"}]}"#);
        assert!(matches!(e, Err(CommandError::InvalidJson(_))));
    }

    #[test]
    fn rejects_extra_fields_and_keystrokes() {
        let e = parse_command_response(
            r#"{"operations":[{"op":"replace_text","expected_text":"a","replacement_text":"b","keys":"ctrl+alt+del"}]}"#,
        );
        assert!(matches!(e, Err(CommandError::InvalidJson(_))));
        let e = parse_command_response(r#"{"operations":[],"extra":"instructions"}"#);
        assert!(matches!(e, Err(CommandError::InvalidJson(_))));
    }

    #[test]
    fn rejects_invalid_json_and_prose() {
        assert!(matches!(
            parse_command_response("Sure! I changed Thursday to Friday."),
            Err(CommandError::InvalidJson(_))
        ));
    }

    #[test]
    fn rejects_control_characters() {
        let e = parse_command_response(
            "{\"operations\":[{\"op\":\"replace_text\",\"expected_text\":\"a\",\"replacement_text\":\"b\\u001b[2J\"}]}",
        );
        assert_eq!(e, Err(CommandError::ControlCharacters("replacement_text")));
        let e = parse_command_response(
            "{\"operations\":[{\"op\":\"insert_text\",\"text\":\"\\t\",\"position\":\"end\"}]}",
        );
        assert_eq!(e, Err(CommandError::ControlCharacters("text")));
    }

    #[test]
    fn rejects_oversized_and_empty_requests() {
        let long = "x".repeat(MAX_OPERATION_TEXT + 1);
        let raw = format!(
            r#"{{"operations":[{{"op":"replace_text","expected_text":"a","replacement_text":"{long}"}}]}}"#
        );
        assert_eq!(
            parse_command_response(&raw),
            Err(CommandError::TextTooLong("replacement_text"))
        );
        assert_eq!(
            parse_command_response(r#"{"operations":[]}"#),
            Err(CommandError::NoOperations)
        );
        let many = r#"{"operations":[{"op":"finish"},{"op":"finish"},{"op":"finish"},{"op":"finish"},{"op":"finish"}]}"#;
        assert_eq!(
            parse_command_response(many),
            Err(CommandError::TooManyOperations(5))
        );
    }

    #[test]
    fn punctuation_op_cannot_change_words() {
        let e = parse_command_response(
            r#"{"operations":[{"op":"replace_punctuation","expected_text":"?","replacement_text":"Friday"}]}"#,
        );
        assert_eq!(e, Err(CommandError::NotPunctuation));
        // Words may anchor the change as long as they stay the same.
        assert!(parse_command_response(
            r#"{"operations":[{"op":"replace_punctuation","expected_text":"park.Then","replacement_text":"park. Then"}]}"#,
        )
        .is_ok());
        assert_eq!(
            parse_command_response(
                r#"{"operations":[{"op":"replace_punctuation","expected_text":"park.Then","replacement_text":"park. then"}]}"#,
            ),
            Err(CommandError::NotPunctuation)
        );
        assert!(parse_command_response(
            r#"{"operations":[{"op":"replace_punctuation","expected_text":"?","replacement_text":".  "}]}"#,
        )
        .is_ok());
    }

    #[test]
    fn insert_before_after_needs_anchor() {
        let e = parse_command_response(
            r#"{"operations":[{"op":"insert_text","text":",","position":"after"}]}"#,
        );
        assert_eq!(e, Err(CommandError::MissingAnchor));
    }

    #[test]
    fn low_confidence_is_rejected() {
        let e = parse_command_response(r#"{"operations":[{"op":"finish"}],"confidence":0.2}"#);
        assert_eq!(e, Err(CommandError::LowConfidence(0.2)));
    }

    #[test]
    fn schema_is_valid_json_with_every_operation() {
        let schema = response_json_schema();
        let ops = schema["properties"]["operations"]["items"]["anyOf"]
            .as_array()
            .unwrap();
        assert_eq!(ops.len(), 11);
    }

    #[test]
    fn memory_requires_users_own_words() {
        assert!(instruction_requests_memory(
            "I meant Striper, S-T-R-I-P-E-R. Remember that."
        ));
        assert!(instruction_requests_memory("always spell it Stephen"));
        assert!(!instruction_requests_memory("change Thursday to Friday"));
        assert!(!instruction_requests_memory("don't remember that spelling"));
    }
    #[test]
    fn paragraph_rewrites_fit_within_bounded_context() {
        let before = "Please join us for lunch tomorrow. ".repeat(25);
        let after = "Come have lunch with us tomorrow! ".repeat(25);
        assert!(before.chars().count() > 500);
        let op = EditOperation::ReplaceText {
            expected_text: before.clone(),
            replacement_text: after.clone(),
            occurrence: None,
        };
        validate_operation(&op).unwrap();
        assert_eq!(
            crate::edit::resolve_operation(&before, &op)
                .unwrap()
                .new_text,
            after
        );
    }
}
