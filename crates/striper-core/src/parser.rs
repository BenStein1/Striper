//! Deterministic interpretation of common spoken correction commands.
//!
//! The everyday commands ("change Thursday to Friday", "scratch that",
//! "undo that", "I meant Stephen, S-T-E-P-H-E-N, remember that") are parsed
//! locally: instantly, offline, and identically every time. Only
//! instructions this parser does not recognise go to the configured LLM.
//! Patterns are anchored to the whole clause so ordinary prose is never
//! half-matched into an edit.

use crate::command::{instruction_requests_memory, EditOperation, InsertPosition};
use crate::edit::find_occurrences;
use crate::text::{capitalize_first, fold, is_all_caps, tokenize};
use regex::Regex;
use std::sync::OnceLock;

/// What the parser knows about the current correction session.
#[derive(Debug, Clone, Default)]
pub struct ParseContext<'a> {
    /// The recently inserted text that may be edited.
    pub region_text: &'a str,
    /// The previous assistant edit in this session, for follow-ups like
    /// "actually, make that Saturday".
    pub last_edit: Option<LastEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastEdit {
    pub replaced_text: String,
    pub inserted_text: String,
}

/// Leading phrases that introduce a correction and carry no meaning.
fn lead_in() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)^\s*(?:(?:okay|ok|um+|uh+|so|hey striper|striper)[\s,.:;!-]+)*(?:correct (?:that|this)|correction)?[\s,.:;!-]*",
        )
        .unwrap()
    })
}

fn spelled_letters() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Three or more single letters separated by hyphens, dots, commas or
    // spaces: "S-T-E-P-H-E-N", "S. T. E. P. H. E. N.", "s t e p h e n".
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b[a-z](?:(?:[\s.,]*[-–][\s.,]*|[\s.,]+)[a-z]\b){2,}\.?").unwrap()
    })
}

/// Spoken names of punctuation and spacing, longest first.
const SYMBOLS: &[(&str, &str)] = &[
    ("new paragraph", "\n\n"),
    ("new line", "\n"),
    ("newline", "\n"),
    ("exclamation point", "!"),
    ("exclamation mark", "!"),
    ("question mark", "?"),
    ("quotation mark", "\""),
    ("double space", "  "),
    ("full stop", "."),
    ("semicolon", ";"),
    ("semi colon", ";"),
    ("apostrophe", "'"),
    ("open paren", "("),
    ("close paren", ")"),
    ("ellipsis", "..."),
    ("period", "."),
    ("comma", ","),
    ("colon", ":"),
    ("hyphen", "-"),
    ("dash", "-"),
    ("quote", "\""),
    ("space", " "),
    ("dot", "."),
];

const NUMBER_WORDS: &[(&str, usize)] = &[
    ("one", 1),
    ("two", 2),
    ("three", 3),
    ("four", 4),
    ("five", 5),
    ("a", 1),
    ("an", 1),
    ("single", 1),
    ("1", 1),
    ("2", 2),
    ("3", 3),
    ("4", 4),
    ("5", 5),
];

/// Map a spoken phrase to the symbol it names: "a question mark" → "?",
/// "two spaces" → "  ". `None` if it is not a symbol name.
pub fn spoken_symbol(phrase: &str) -> Option<String> {
    let p = fold(phrase.trim().trim_matches(|c: char| c == '.' || c == ','));
    let p = p
        .strip_prefix("the ")
        .or_else(|| p.strip_prefix("that "))
        .or_else(|| p.strip_prefix("this "))
        .unwrap_or(&p)
        .to_string();
    let mut count = 1usize;
    let mut rest = p.as_str();
    for (word, n) in NUMBER_WORDS {
        if let Some(r) = rest.strip_prefix(&format!("{word} ")) {
            count = *n;
            rest = r;
            break;
        }
    }
    let singular = rest.strip_suffix('s').filter(|_| count > 1).unwrap_or(rest);
    for (name, symbol) in SYMBOLS {
        if rest == *name || singular == *name {
            return Some(symbol.repeat(count));
        }
    }
    // Already a literal symbol ("?", "  ").
    if !phrase.is_empty() && phrase.chars().all(|c| !c.is_alphanumeric() && c != ' ') {
        return Some(phrase.to_string());
    }
    None
}

/// Strip quotes, trailing punctuation and filler from a captured phrase.
fn clean_capture(s: &str) -> String {
    let t = s.trim().trim_end_matches(['.', ',', '!', ';', ':']).trim();
    let t = t.trim_matches(|c| matches!(c, '"' | '“' | '”' | '\'' | '‘' | '’'));
    t.trim().to_string()
}

/// The text a spoken target refers to: a symbol name, or the words as said.
fn spoken_target(s: &str) -> String {
    spoken_symbol(s).unwrap_or_else(|| clean_capture(s))
}

/// Choose the variant of a spoken target that actually exists in the
/// region ("the meeting" vs "meeting").
fn locate(region: &str, spoken: &str) -> String {
    let target = spoken_target(spoken);
    let in_region = |candidate: &str| {
        find_occurrences(region, candidate)
            .first()
            .map(|r| region[r.clone()].to_string())
    };
    if let Some(found) = in_region(&target) {
        return found;
    }
    for article in ["the ", "a ", "an ", "that ", "this "] {
        if let Some(stripped) = fold(&target).strip_prefix(article) {
            let candidate = &target[target.len() - stripped.len()..];
            if let Some(found) = in_region(candidate) {
                return found;
            }
        }
    }
    target
}

/// Collapse a spelled-letter run into a word, cased like `spoken` when the
/// letters match it, otherwise as a capitalized name.
fn word_from_letters(letters_raw: &str, spoken: Option<&str>) -> String {
    let letters: String = letters_raw.chars().filter(|c| c.is_alphabetic()).collect();
    let lower = letters.to_lowercase();
    if let Some(sp) = spoken {
        let sp = clean_capture(sp);
        if fold(&sp) == lower {
            return sp;
        }
    }
    if letters.chars().all(|c| c.is_lowercase()) {
        lower
    } else {
        capitalize_first(&lower)
    }
}

fn soundex(word: &str) -> String {
    let mut chars = word
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .map(|c| c.to_ascii_lowercase());
    let Some(first) = chars.next() else {
        return String::new();
    };
    let code = |c: char| match c {
        'b' | 'f' | 'p' | 'v' => '1',
        'c' | 'g' | 'j' | 'k' | 'q' | 's' | 'x' | 'z' => '2',
        'd' | 't' => '3',
        'l' => '4',
        'm' | 'n' => '5',
        'r' => '6',
        _ => '0',
    };
    let mut out = String::from(first.to_ascii_uppercase());
    let mut last = code(first);
    for c in chars {
        let d = code(c);
        if d != '0' && d != last {
            out.push(d);
        }
        if c != 'h' && c != 'w' {
            last = d;
        }
        if out.len() == 4 {
            break;
        }
    }
    while out.len() < 4 {
        out.push('0');
    }
    out
}

fn similarity(a: &str, b: &str) -> f64 {
    let (fa, fb) = (fold(a), fold(b));
    let jw = strsim::jaro_winkler(&fa, &fb);
    let compact = |s: &str| {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
    };
    let jw_compact = strsim::jaro_winkler(&compact(&fa), &compact(&fb));
    let phonetic = if soundex(&compact(&fa)) == soundex(&compact(&fb)) {
        0.08
    } else {
        0.0
    };
    jw.max(jw_compact) + phonetic
}

/// The span of the region the user most plausibly meant by `word`: the
/// most similar single word or adjacent word pair. Refuses to guess when
/// nothing is close or two different spans tie.
pub fn find_misrecognized(region: &str, word: &str) -> Option<String> {
    const THRESHOLD: f64 = 0.78;
    let tokens = tokenize(region);
    let mut scored: Vec<(f64, String)> = Vec::new();
    for i in 0..tokens.len() {
        for len in 1..=2 {
            if i + len > tokens.len() {
                break;
            }
            let span = &region[tokens[i].start..tokens[i + len - 1].end];
            if span == word {
                continue;
            }
            scored.push((similarity(span, word), span.to_string()));
        }
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let (best_score, best) = scored.first()?.clone();
    if best_score < THRESHOLD {
        return None;
    }
    if let Some((second_score, second)) = scored.iter().find(|(_, s)| fold(s) != fold(&best)) {
        // A two-word span containing the best word is the same choice.
        let overlapping = second.contains(best.as_str()) || best.contains(second.as_str());
        if !overlapping && best_score - second_score < 0.04 {
            return None;
        }
    }
    Some(best)
}

struct Patterns {
    scratch: Regex,
    undo: Regex,
    finish: Regex,
    cancel: Regex,
    forget: Regex,
    replace: Regex,
    follow_up: Regex,
    spelling: Regex,
    delete: Regex,
    paragraph_after: Regex,
    insert_relative: Regex,
    insert_edge: Regex,
    capitalize: Regex,
    lowercase: Regex,
    remember_only: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| {
        let re = |s: &str| Regex::new(&format!("(?i)^{s}$")).unwrap();
        Patterns {
            scratch: re(r"(?:scratch|strike|delete|erase|remove) (?:that|it|the last (?:dictation|sentence|phrase|thing))"),
            undo: re(r"undo(?: that| it| this| the last (?:edit|change|correction))?|put it back|change it back"),
            finish: re(r"done|finished|i'?m done|that'?s all|that'?s it|all done|thank you|thanks|good|perfect|that'?s good|that'?s fine"),
            cancel: re(r"cancel|never ?mind|forget it|stop|quit"),
            forget: re(r"(?:don'?t|do not) (?:remember|learn|save) (?:that|this|it)(?: spelling| word| name)?|forget (?:that|this|the) (?:spelling|word|name)(?: of (?P<w1>.+))?|(?:forget|unlearn|stop correcting) (?:the spelling of )?(?P<w2>.+)"),
            replace: re(r"(?:change|replace|switch|swap|correct) (?P<from>.+?) (?:to|with|into|for|by) (?P<to>.+)"),
            follow_up: re(r"(?:actually|no|sorry|oops)?[\s,]*(?:make|change) (?:that|it) (?:to |into )?(?P<to>.+)|(?:actually|no|sorry)[\s,]+(?:it'?s |it should be |make it )?(?P<to2>.+)"),
            spelling: re(r"(?:i said|i meant|i mean|it'?s|it is|it should be|that should be|should be|that'?s|that is|the word is|it'?s spelled|that'?s spelled|spell(?:ed)? it|spell it as|my (?:first |last |middle |sur)?name is|(?:his|her|their|the) (?:first |last )?name is)(?:[\s,:]+(?P<word>.+))?"),
            delete: re(r"(?:delete|remove|erase|take out|drop) (?:the word |the words )?(?P<what>.+)"),
            paragraph_after: re(r"(?:(?:start|begin|add|insert|put|make)(?: in)? (?:a )?)?(?:new paragraph|paragraph break) (?:after|following) (?P<anchor>.+)"),
            insert_relative: re(r"(?:add|insert|put|type) (?P<what>.+?) (?P<pos>after|before) (?P<anchor>.+)"),
            insert_edge: re(r"(?:add|insert|put|type) (?P<what>.+?) (?:at the (?P<edge>end|beginning|start)(?: of (?:it|that|the sentence))?|after (?:it|that|the sentence))"),
            capitalize: re(r"(?:capitalize|capitalise|cap) (?P<what>.+)"),
            lowercase: re(r"(?:lowercase|lower case|uncapitalize|no caps) (?P<what>.+)"),
            remember_only: re(r"(?:please )?(?:remember|learn|save) (?:that|it|this)(?: spelling| word| name)?(?: please)?|always spell it that way"),
        }
    })
}

/// Phrases asking for memory, removed before matching the edit itself.
fn memory_phrases() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)[\s,.;]*(?:and |please )*(?:remember (?:that|it|this)(?: spelling| word| name)?|always spell it that way|from now on|learn (?:that|it|this))(?: please)?[\s,.;!]*").unwrap()
    })
}

/// Parse an instruction into operations, or `None` if it is not one of the
/// recognised forms (the caller may then consult the LLM).
pub fn parse_instruction(instruction: &str, ctx: &ParseContext<'_>) -> Option<Vec<EditOperation>> {
    let remember = instruction_requests_memory(instruction);
    let mut text = lead_in().replace(instruction, "").to_string();

    // Pull out spelled letters first: their dots and commas would otherwise
    // split clauses.
    let spelled = spelled_letters()
        .find(&text)
        .map(|m| (m.as_str().to_string(), m.start(), m.end()));
    if let Some((_, s, e)) = &spelled {
        text = format!("{} {}", &text[..*s], &text[*e..]);
    }
    let spelled_raw = spelled.map(|(raw, _, _)| raw);

    let p = patterns();
    let whole = clean_capture(&text);
    if remember && spelled_raw.is_none() && p.remember_only.is_match(&whole) {
        return remember_last_edit(ctx);
    }
    if p.forget.is_match(&whole) {
        let caps = p.forget.captures(&whole)?;
        let target = caps
            .name("w1")
            .or_else(|| caps.name("w2"))
            .map(|m| clean_capture(m.as_str()))
            .filter(|w| !w.is_empty() && !matches!(fold(w).as_str(), "it" | "that" | "this"));
        return Some(vec![EditOperation::ForgetSpelling { text: target }]);
    }

    let stripped = memory_phrases().replace_all(&text, " ").to_string();
    let clauses: Vec<String> = stripped
        .split(['.', ';', '!', '?'])
        .flat_map(|c| c.split(", then "))
        .map(clean_capture)
        .filter(|c| !c.is_empty())
        .collect();

    // A bare spelled word after "I meant"/"my name is" leaves its clause
    // empty-ish; treat the spelled letters as the word itself.
    if clauses.is_empty() {
        if let Some(raw) = &spelled_raw {
            return spelling_correction(ctx, &word_from_letters(raw, None), remember);
        }
        return if remember {
            remember_last_edit(ctx)
        } else {
            None
        };
    }

    let mut ops = Vec::new();
    for clause in &clauses {
        let op = parse_clause(clause, ctx, spelled_raw.as_deref(), remember)?;
        ops.extend(op);
    }
    if remember
        && !ops
            .iter()
            .any(|o| matches!(o, EditOperation::RememberSpelling { .. }))
        && ops.len() == 1
    {
        if let EditOperation::ReplaceText {
            expected_text,
            replacement_text,
            ..
        } = &ops[0]
        {
            if !replacement_text.trim().is_empty() {
                ops.push(remember_op(replacement_text, expected_text));
            }
        }
    }
    Some(ops)
}

fn remember_op(preferred: &str, alias: &str) -> EditOperation {
    EditOperation::RememberSpelling {
        preferred_text: preferred.to_string(),
        aliases: if alias.is_empty() || fold(alias) == fold(preferred) && alias == preferred {
            vec![]
        } else {
            vec![alias.to_string()]
        },
        context_words: vec![],
        always: false,
    }
}

fn remember_last_edit(ctx: &ParseContext<'_>) -> Option<Vec<EditOperation>> {
    let last = ctx.last_edit.as_ref()?;
    if last.inserted_text.trim().is_empty() {
        return None;
    }
    Some(vec![remember_op(
        last.inserted_text.trim(),
        last.replaced_text.trim(),
    )])
}

fn spelling_correction(
    ctx: &ParseContext<'_>,
    word: &str,
    remember: bool,
) -> Option<Vec<EditOperation>> {
    if word.is_empty() {
        return None;
    }
    let mut ops = Vec::new();
    match find_misrecognized(ctx.region_text, word) {
        Some(misheard) => {
            // Spelled letters carry no case; follow the word being fixed.
            let word = if word.chars().all(|c| !c.is_uppercase())
                && crate::text::is_capitalized(&misheard)
            {
                capitalize_first(word)
            } else {
                word.to_string()
            };
            let word = word.as_str();
            ops.push(EditOperation::ReplaceText {
                expected_text: misheard.clone(),
                replacement_text: word.to_string(),
                occurrence: None,
            });
            if remember {
                ops.push(remember_op(word, &misheard));
            }
        }
        None if remember => ops.push(remember_op(word, "")),
        None => {
            ops.push(EditOperation::Clarify {
                question: format!("Which word should be \"{word}\"?"),
            });
        }
    }
    Some(ops)
}

fn parse_clause(
    clause: &str,
    ctx: &ParseContext<'_>,
    spelled: Option<&str>,
    remember: bool,
) -> Option<Vec<EditOperation>> {
    let p = patterns();
    let region = ctx.region_text;
    if p.scratch.is_match(clause) {
        return Some(vec![EditOperation::DeleteRecentDictation]);
    }
    if p.undo.is_match(clause) {
        return Some(vec![EditOperation::UndoLastEdit]);
    }
    if p.finish.is_match(clause) {
        return Some(vec![EditOperation::Finish]);
    }
    if p.cancel.is_match(clause) {
        return Some(vec![EditOperation::Cancel]);
    }
    if let Some(c) = p.spelling.captures(clause) {
        let spoken = c.name("word").map(|m| m.as_str()).unwrap_or("");
        let spoken = spoken
            .trim()
            .trim_end_matches([',', '.'])
            .trim_end_matches("spelled")
            .trim_end_matches([',', ' ']);
        let word = match spelled {
            Some(raw) => word_from_letters(raw, Some(spoken).filter(|s| !s.is_empty())),
            None => clean_capture(spoken),
        };
        if word.split_whitespace().count() > 4 {
            return None;
        }
        return spelling_correction(ctx, &word, remember);
    }
    if let Some(c) = p.paragraph_after.captures(clause) {
        let anchor = locate(region, c.name("anchor")?.as_str());
        return Some(vec![EditOperation::InsertParagraphBreaks {
            after: vec![anchor],
        }]);
    }
    if let Some(c) = p.insert_edge.captures(clause) {
        let what = spoken_target(c.name("what")?.as_str());
        let position = match c.name("edge").map(|m| fold(m.as_str())) {
            Some(e) if e == "beginning" || e == "start" => InsertPosition::Start,
            _ => InsertPosition::End,
        };
        return Some(vec![EditOperation::InsertText {
            text: what,
            position,
            anchor_text: None,
        }]);
    }
    if let Some(c) = p.insert_relative.captures(clause) {
        let what = spoken_target(c.name("what")?.as_str());
        let anchor = locate(region, c.name("anchor")?.as_str());
        let position = if fold(c.name("pos")?.as_str()) == "before" {
            InsertPosition::Before
        } else {
            InsertPosition::After
        };
        return Some(vec![EditOperation::InsertText {
            text: what,
            position,
            anchor_text: Some(anchor),
        }]);
    }
    if let Some(c) = p.replace.captures(clause) {
        let from = locate(region, c.name("from")?.as_str());
        let to_raw = c.name("to")?.as_str();
        let mut to = spoken_symbol(to_raw).unwrap_or_else(|| clean_capture(to_raw));
        if let Some(raw) = spelled {
            to = word_from_letters(raw, Some(&to));
        }
        let punctuation_only = |s: &str| !s.is_empty() && s.chars().all(|ch| !ch.is_alphanumeric());
        let op = if punctuation_only(&from) && (to.is_empty() || punctuation_only(&to)) {
            EditOperation::ReplacePunctuation {
                expected_text: from,
                replacement_text: to,
                occurrence: None,
            }
        } else {
            EditOperation::ReplaceText {
                expected_text: from,
                replacement_text: to,
                occurrence: None,
            }
        };
        return Some(vec![op]);
    }
    if let Some(c) = p.follow_up.captures(clause) {
        let last = ctx.last_edit.as_ref()?;
        let to_raw = c.name("to").or_else(|| c.name("to2"))?.as_str();
        let mut to = spoken_symbol(to_raw).unwrap_or_else(|| clean_capture(to_raw));
        if let Some(raw) = spelled {
            to = word_from_letters(raw, Some(&to));
        }
        if last.inserted_text.is_empty() || to.is_empty() || to.split_whitespace().count() > 6 {
            return None;
        }
        return Some(vec![EditOperation::ReplaceText {
            expected_text: last.inserted_text.clone(),
            replacement_text: to,
            occurrence: None,
        }]);
    }
    if let Some(c) = p.capitalize.captures(clause) {
        let what = locate(region, c.name("what")?.as_str());
        return Some(vec![EditOperation::ReplaceText {
            replacement_text: capitalize_first(&what),
            expected_text: what,
            occurrence: None,
        }]);
    }
    if let Some(c) = p.lowercase.captures(clause) {
        let what = locate(region, c.name("what")?.as_str());
        let lowered = if is_all_caps(&what) {
            what.to_lowercase()
        } else {
            let mut ch = what.chars();
            ch.next()
                .map(|f| f.to_lowercase().collect::<String>() + ch.as_str())
                .unwrap_or_default()
        };
        return Some(vec![EditOperation::ReplaceText {
            expected_text: what,
            replacement_text: lowered,
            occurrence: None,
        }]);
    }
    if let Some(c) = p.delete.captures(clause) {
        let what = locate(region, c.name("what")?.as_str());
        return Some(vec![EditOperation::ReplaceText {
            expected_text: what,
            replacement_text: String::new(),
            occurrence: None,
        }]);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use EditOperation::*;

    fn ctx(region: &str) -> ParseContext<'_> {
        ParseContext {
            region_text: region,
            last_edit: None,
        }
    }

    fn rt(from: &str, to: &str) -> EditOperation {
        ReplaceText {
            expected_text: from.into(),
            replacement_text: to.into(),
            occurrence: None,
        }
    }

    #[test]
    fn change_x_to_y() {
        let c = ctx("I'll see you on Thursday.");
        assert_eq!(
            parse_instruction("Correct that. Change Thursday to Friday.", &c),
            Some(vec![rt("Thursday", "Friday")])
        );
        assert_eq!(
            parse_instruction("replace thursday with Friday", &c),
            Some(vec![rt("Thursday", "Friday")])
        );
    }

    #[test]
    fn follow_up_uses_previous_edit() {
        let mut c = ctx("I'll see you on Friday.");
        c.last_edit = Some(LastEdit {
            replaced_text: "Thursday".into(),
            inserted_text: "Friday".into(),
        });
        assert_eq!(
            parse_instruction("Actually, make that Saturday.", &c),
            Some(vec![rt("Friday", "Saturday")])
        );
        assert_eq!(
            parse_instruction("make it Sunday", &c),
            Some(vec![rt("Friday", "Sunday")])
        );
        assert_eq!(
            parse_instruction("No, Saturday.", &c),
            Some(vec![rt("Friday", "Saturday")])
        );
    }

    #[test]
    fn follow_up_without_history_is_unrecognised() {
        assert_eq!(parse_instruction("make that Saturday", &ctx("hello")), None);
    }

    #[test]
    fn scratch_undo_finish_cancel() {
        let c = ctx("x");
        assert_eq!(
            parse_instruction("Scratch that.", &c),
            Some(vec![DeleteRecentDictation])
        );
        assert_eq!(parse_instruction("undo that", &c), Some(vec![UndoLastEdit]));
        assert_eq!(parse_instruction("Done.", &c), Some(vec![Finish]));
        assert_eq!(parse_instruction("never mind", &c), Some(vec![Cancel]));
    }

    #[test]
    fn punctuation_by_name() {
        let c = ctx("Are you coming?");
        assert_eq!(
            parse_instruction("Replace that question mark with a period.", &c),
            Some(vec![ReplacePunctuation {
                expected_text: "?".into(),
                replacement_text: ".".into(),
                occurrence: None
            }])
        );
        assert_eq!(
            parse_instruction("put two spaces after the sentence", &c),
            Some(vec![InsertText {
                text: "  ".into(),
                position: InsertPosition::End,
                anchor_text: None
            }])
        );
        assert_eq!(
            parse_instruction("add a comma after coming", &c),
            Some(vec![InsertText {
                text: ",".into(),
                position: InsertPosition::After,
                anchor_text: Some("coming".into())
            }])
        );
    }

    #[test]
    fn spoken_symbols() {
        assert_eq!(spoken_symbol("a question mark").as_deref(), Some("?"));
        assert_eq!(spoken_symbol("three spaces").as_deref(), Some("   "));
        assert_eq!(spoken_symbol("the period").as_deref(), Some("."));
        assert_eq!(spoken_symbol("Friday"), None);
    }

    // PLAN.md §0 acceptance phrasing.
    #[test]
    fn striper_teaching_phrase() {
        let c = ctx("I opened striped.");
        let ops = parse_instruction(
            "Correct that; I meant Striper, S-T-R-I-P-E-R; remember that.",
            &c,
        )
        .unwrap();
        assert_eq!(ops[0], rt("striped", "Striper"));
        assert_eq!(
            ops[1],
            RememberSpelling {
                preferred_text: "Striper".into(),
                aliases: vec!["striped".into()],
                context_words: vec![],
                always: false
            }
        );
    }

    #[test]
    fn stephen_teaching_phrase() {
        let c = ctx("Hi, my name is Steven.");
        let ops = parse_instruction(
            "Correct that. My name is Stephen, S-T-E-P-H-E-N. Remember that.",
            &c,
        )
        .unwrap();
        assert_eq!(ops[0], rt("Steven", "Stephen"));
        assert!(
            matches!(&ops[1], RememberSpelling { preferred_text, aliases, .. } if preferred_text == "Stephen" && aliases == &vec!["Steven".to_string()])
        );
    }

    #[test]
    fn spelled_letters_variants() {
        let c = ctx("call Steven tomorrow");
        for spoken in [
            "I said Stephen S. T. E. P. H. E. N.",
            "I meant s t e p h e n",
            "it's spelled S, T, E, P, H, E, N",
        ] {
            let ops = parse_instruction(spoken, &c).unwrap();
            assert_eq!(ops[0], rt("Steven", "Stephen"), "{spoken}");
        }
    }

    #[test]
    fn spelling_without_remember_does_not_persist() {
        let c = ctx("I opened striped.");
        let ops = parse_instruction("I meant Striper", &c).unwrap();
        assert_eq!(ops, vec![rt("striped", "Striper")]);
    }

    #[test]
    fn spelling_with_no_similar_word_asks() {
        let ops = parse_instruction("I meant Stephen", &ctx("The weather is nice")).unwrap();
        assert!(matches!(ops[0], Clarify { .. }));
    }

    #[test]
    fn remember_after_a_correction() {
        let mut c = ctx("I opened Striper.");
        c.last_edit = Some(LastEdit {
            replaced_text: "striped".into(),
            inserted_text: "Striper".into(),
        });
        let ops = parse_instruction("Remember that.", &c).unwrap();
        assert!(
            matches!(&ops[0], RememberSpelling { preferred_text, aliases, .. } if preferred_text == "Striper" && aliases == &vec!["striped".to_string()])
        );
    }

    #[test]
    fn change_with_remember_learns_it() {
        let c = ctx("launch stripper");
        let ops = parse_instruction("change stripper to Striper and remember that", &c).unwrap();
        assert_eq!(ops[0], rt("stripper", "Striper"));
        assert!(matches!(&ops[1], RememberSpelling { .. }));
    }

    #[test]
    fn forget_spelling() {
        let c = ctx("x");
        assert_eq!(
            parse_instruction("Don't remember that spelling.", &c),
            Some(vec![ForgetSpelling { text: None }])
        );
        assert_eq!(
            parse_instruction("forget the spelling of Stephen", &c),
            Some(vec![ForgetSpelling {
                text: Some("Stephen".into())
            }])
        );
    }

    #[test]
    fn new_paragraph_after_a_phrase() {
        let region = "Thanks for coming. See you tomorrow. Love, Dad";
        for said in [
            "start a new paragraph after see you tomorrow",
            "new paragraph after 'see you tomorrow'",
            "Insert a paragraph break after see you tomorrow.",
        ] {
            assert_eq!(
                parse_instruction(said, &ctx(region)),
                Some(vec![EditOperation::InsertParagraphBreaks {
                    after: vec!["See you tomorrow".into()]
                }]),
                "{said}"
            );
        }
    }

    #[test]
    fn delete_and_capitalize() {
        let c = ctx("this is a very big deal");
        assert_eq!(
            parse_instruction("delete the word very", &c),
            Some(vec![rt("very", "")])
        );
        assert_eq!(
            parse_instruction("capitalize big", &c),
            Some(vec![rt("big", "Big")])
        );
    }

    #[test]
    fn articles_are_dropped_when_needed() {
        let c = ctx("Bring meeting notes");
        assert_eq!(
            parse_instruction("change the meeting to the party", &c),
            Some(vec![rt("meeting", "the party")])
        );
    }

    #[test]
    fn unrecognised_prose_goes_to_the_llm() {
        let c = ctx("I'll see you on Thursday.");
        assert_eq!(
            parse_instruction("make the whole thing sound more formal", &c),
            None
        );
        assert_eq!(parse_instruction("hmm", &c), None);
    }

    #[test]
    fn misrecognition_finder() {
        assert_eq!(
            find_misrecognized("I opened striped today", "Striper").as_deref(),
            Some("striped")
        );
        assert_eq!(
            find_misrecognized("launch stripper", "Striper").as_deref(),
            Some("stripper")
        );
        assert_eq!(find_misrecognized("nothing alike here", "Stephen"), None);
    }
}

/// Session controls stay instantaneous/offline. With AI configured, all other
/// instructions go to the model, even if a regex would have matched them.
pub fn local_operations_for_mode(
    parsed: Option<Vec<EditOperation>>,
    ai_configured: bool,
) -> Option<Vec<EditOperation>> {
    parsed.filter(|ops| {
        !ai_configured
            || (!ops.is_empty()
                && ops.iter().all(|op| {
                    matches!(
                        op,
                        EditOperation::Cancel | EditOperation::Finish | EditOperation::UndoLastEdit
                    )
                }))
    })
}

#[cfg(test)]
mod routing_tests {
    use super::*;
    #[test]
    fn ai_owns_edits_but_not_session_controls() {
        let ctx = ParseContext {
            region_text: "Meet Thursday",
            last_edit: None,
        };
        for instruction in ["change Thursday to Friday", "scratch that"] {
            let parsed = parse_instruction(instruction, &ctx);
            assert!(parsed.is_some());
            assert!(local_operations_for_mode(parsed.clone(), false).is_some());
            assert!(local_operations_for_mode(parsed, true).is_none());
        }
        for instruction in ["undo that", "done", "cancel"] {
            assert!(
                local_operations_for_mode(parse_instruction(instruction, &ctx), true).is_some()
            );
        }
        assert!(local_operations_for_mode(None, false).is_none());
        assert!(local_operations_for_mode(Some(vec![]), true).is_none());
    }
}
