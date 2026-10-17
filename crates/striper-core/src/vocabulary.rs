//! Learned vocabulary: user-taught spellings applied deterministically to a
//! transcript before it is inserted (Lane A, PLAN.md §6).
//!
//! The central safety rule is that an alias which is also a legitimate word
//! ("striped", "stripper", "Steven") is never replaced blindly. A
//! [`MatchPolicy::Contextual`] entry only fires when the surrounding text
//! carries a positive signal: one of the entry's context words nearby, or a
//! normally-lowercase alias that the recognizer capitalized mid-sentence (the
//! recognizer's own hint that it heard a proper noun). Anything else is left
//! exactly as transcribed.

use crate::text::{fold, is_capitalized, match_case, opens_sentence, tokenize, Token};
use serde::{Deserialize, Serialize};

/// How eagerly an entry's aliases are replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchPolicy {
    /// Replace only when the context supports it. The default for anything
    /// learned from a correction.
    Contextual,
    /// Replace every occurrence of an alias. Only for aliases the user said
    /// should always be rewritten.
    Always,
}

/// Where an entry applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Scope {
    Global,
    /// Only while dictating into this application (lowercased process name,
    /// e.g. `winword.exe`, `kate`).
    App(String),
}

impl Scope {
    pub fn applies_to(&self, app_id: Option<&str>) -> bool {
        match self {
            Scope::Global => true,
            Scope::App(app) => app_id.is_some_and(|id| id.eq_ignore_ascii_case(app)),
        }
    }
}

/// One user-taught preference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VocabularyEntry {
    pub id: i64,
    /// The spelling to produce ("Striper", "Stephen").
    pub preferred_text: String,
    /// What the recognizer produces instead ("striped", "stripper", "Steven").
    pub aliases: Vec<String>,
    /// Optional pronunciation note, e.g. the spelled letters the user gave.
    pub spoken_hint: Option<String>,
    pub policy: MatchPolicy,
    /// Folded words whose presence near an alias justifies a replacement.
    pub context_words: Vec<String>,
    /// Folded words whose presence near an alias vetoes a replacement.
    pub negative_context_words: Vec<String>,
    pub scope: Scope,
    /// Entries are only applied once the user confirmed them.
    pub user_confirmed: bool,
    pub enabled: bool,
    /// Also send `preferred_text` to the recognizer as a vocabulary hint.
    pub use_as_asr_hint: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl VocabularyEntry {
    pub fn is_active(&self) -> bool {
        self.enabled && self.user_confirmed
    }
}

/// Context for applying vocabulary to one transcript.
#[derive(Debug, Clone, Default)]
pub struct ApplyContext<'a> {
    /// Lowercased process name of the focused application, when known.
    pub app_id: Option<&'a str>,
}

/// Why a replacement fired; kept for diagnostics and the history view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum MatchReason {
    Always,
    ContextWord(String),
    CapitalizedMidSentence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedRule {
    pub entry_id: i64,
    pub original: String,
    pub replacement: String,
    pub reason: MatchReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VocabularyOutcome {
    pub text: String,
    pub applied: Vec<AppliedRule>,
}

/// How many words either side of a match are searched for context words.
pub const CONTEXT_WINDOW_WORDS: usize = 10;

struct Candidate {
    start_token: usize,
    end_token: usize, // exclusive
    entry_index: usize,
    reason: MatchReason,
}

/// Apply every active, in-scope entry to `text`.
pub fn apply_vocabulary(
    text: &str,
    entries: &[VocabularyEntry],
    ctx: &ApplyContext<'_>,
) -> VocabularyOutcome {
    let tokens = tokenize(text);
    let folded: Vec<String> = tokens.iter().map(|t| fold(t.text)).collect();
    let mut candidates: Vec<Candidate> = Vec::new();

    for (entry_index, entry) in entries.iter().enumerate() {
        if !entry.is_active() || !entry.scope.applies_to(ctx.app_id) {
            continue;
        }
        for alias in match_forms(entry) {
            let alias_tokens: Vec<String> = tokenize(&alias).iter().map(|t| fold(t.text)).collect();
            if alias_tokens.is_empty() || alias_tokens.len() > tokens.len() {
                continue;
            }
            let lowercase_alias = alias == fold(&alias);
            for start in 0..=tokens.len() - alias_tokens.len() {
                let end = start + alias_tokens.len();
                if folded[start..end] != alias_tokens[..] {
                    continue;
                }
                let original = &text[tokens[start].start..tokens[end - 1].end];
                if original == entry.preferred_text {
                    continue;
                }
                if let Some(reason) = decide(
                    entry,
                    &tokens,
                    &folded,
                    text,
                    start,
                    end,
                    original,
                    lowercase_alias,
                ) {
                    candidates.push(Candidate {
                        start_token: start,
                        end_token: end,
                        entry_index,
                        reason,
                    });
                }
            }
        }
    }

    if candidates.is_empty() {
        return VocabularyOutcome {
            text: text.to_string(),
            applied: Vec::new(),
        };
    }

    // Longest match first, then earliest; drop overlaps.
    candidates.sort_by(|a, b| {
        (b.end_token - b.start_token)
            .cmp(&(a.end_token - a.start_token))
            .then(a.start_token.cmp(&b.start_token))
    });
    let mut taken = vec![false; tokens.len()];
    let mut chosen: Vec<Candidate> = Vec::new();
    for c in candidates {
        if taken[c.start_token..c.end_token].iter().any(|t| *t) {
            continue;
        }
        taken[c.start_token..c.end_token]
            .iter_mut()
            .for_each(|t| *t = true);
        chosen.push(c);
    }
    chosen.sort_by_key(|c| c.start_token);

    let mut out = String::with_capacity(text.len());
    let mut applied = Vec::new();
    let mut cursor = 0;
    for c in chosen {
        let entry = &entries[c.entry_index];
        let start = tokens[c.start_token].start;
        let end = tokens[c.end_token - 1].end;
        let original = &text[start..end];
        let replacement = match_case(original, &entry.preferred_text, opens_sentence(text, start));
        out.push_str(&text[cursor..start]);
        out.push_str(&replacement);
        cursor = end;
        applied.push(AppliedRule {
            entry_id: entry.id,
            original: original.to_string(),
            replacement,
            reason: c.reason,
        });
    }
    out.push_str(&text[cursor..]);
    VocabularyOutcome { text: out, applied }
}

/// The surface forms that trigger an entry: its aliases plus, when the
/// preferred spelling has capitals, its lowercase form (a pure case fix).
fn match_forms(entry: &VocabularyEntry) -> Vec<String> {
    let mut forms: Vec<String> = entry
        .aliases
        .iter()
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
        .collect();
    let lower = entry.preferred_text.to_lowercase();
    if lower != entry.preferred_text && !forms.iter().any(|f| fold(f) == fold(&lower)) {
        forms.push(lower);
    }
    forms
}

#[allow(clippy::too_many_arguments)]
fn decide(
    entry: &VocabularyEntry,
    tokens: &[Token<'_>],
    folded: &[String],
    text: &str,
    start: usize,
    end: usize,
    original: &str,
    lowercase_alias: bool,
) -> Option<MatchReason> {
    let lo = start.saturating_sub(CONTEXT_WINDOW_WORDS);
    let hi = (end + CONTEXT_WINDOW_WORDS).min(tokens.len());
    let nearby = || (lo..hi).filter(move |i| *i < start || *i >= end);

    if nearby().any(|i| entry.negative_context_words.iter().any(|w| *w == folded[i])) {
        return None;
    }
    if entry.policy == MatchPolicy::Always {
        return Some(MatchReason::Always);
    }
    if let Some(i) = nearby().find(|i| entry.context_words.iter().any(|w| *w == folded[*i])) {
        return Some(MatchReason::ContextWord(folded[i].clone()));
    }
    // The recognizer capitalizing a normally-lowercase word mid-sentence is
    // its own signal that it heard a name. Never used for aliases that are
    // capitalized anyway (another person called "Steven").
    if lowercase_alias && is_capitalized(original) && !opens_sentence(text, tokens[start].start) {
        return Some(MatchReason::CapitalizedMidSentence);
    }
    None
}

/// Words never worth keeping as context: too common to discriminate.
const STOPWORDS: &[&str] = &[
    "a", "about", "after", "again", "all", "also", "am", "an", "and", "any", "are", "as", "at",
    "be", "because", "been", "before", "being", "but", "by", "can", "could", "did", "do", "does",
    "doing", "don't", "for", "from", "had", "has", "have", "having", "he", "her", "here", "hers",
    "him", "his", "how", "i", "i'll", "i'm", "i've", "if", "in", "into", "is", "it", "it's", "its",
    "just", "let's", "me", "more", "most", "my", "no", "not", "now", "of", "on", "once", "only",
    "or", "other", "our", "out", "over", "own", "same", "she", "should", "so", "some", "such",
    "than", "that", "that's", "the", "their", "them", "then", "there", "these", "they", "this",
    "those", "through", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what",
    "when", "where", "which", "while", "who", "whom", "why", "will", "with", "would", "you",
    "your", "yours", "okay", "ok", "yeah", "yes", "really", "like", "get", "got", "one", "name",
    "first", "last", "called", "call", "meant", "said", "say",
];

pub fn is_stopword(word: &str) -> bool {
    STOPWORDS.contains(&fold(word).as_str())
}

/// Context words to remember when the user corrects `misheard` (byte range
/// in `text`): nearby content words, which later distinguish this meaning
/// of the word from its ordinary uses.
pub fn learn_context_words(
    text: &str,
    range: std::ops::Range<usize>,
    exclude: &[&str],
) -> Vec<String> {
    const RADIUS: usize = 4;
    const MAX: usize = 6;
    let tokens = tokenize(text);
    let excluded: Vec<String> = exclude.iter().map(|w| fold(w)).collect();
    let Some(first) = tokens.iter().position(|t| t.end > range.start) else {
        return Vec::new();
    };
    let last = tokens
        .iter()
        .rposition(|t| t.start < range.end)
        .unwrap_or(first);
    let lo = first.saturating_sub(RADIUS);
    let hi = (last + 1 + RADIUS).min(tokens.len());
    let mut words: Vec<String> = Vec::new();
    // Nearest first, so the cap keeps the most relevant words.
    let mut order: Vec<usize> = (lo..hi).filter(|i| *i < first || *i > last).collect();
    order.sort_by_key(|i| if *i < first { first - i } else { i - last });
    for i in order {
        let w = fold(tokens[i].text);
        if w.chars().count() < 3
            || w.chars().all(|c| c.is_numeric())
            || is_stopword(&w)
            || excluded.contains(&w)
            || words.contains(&w)
        {
            continue;
        }
        words.push(w);
        if words.len() == MAX {
            break;
        }
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(preferred: &str, aliases: &[&str], context: &[&str]) -> VocabularyEntry {
        VocabularyEntry {
            id: 1,
            preferred_text: preferred.into(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            spoken_hint: None,
            policy: MatchPolicy::Contextual,
            context_words: context.iter().map(|s| s.to_string()).collect(),
            negative_context_words: vec![],
            scope: Scope::Global,
            user_confirmed: true,
            enabled: true,
            use_as_asr_hint: true,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn apply(text: &str, entries: &[VocabularyEntry]) -> String {
        apply_vocabulary(text, entries, &ApplyContext::default()).text
    }

    fn striper() -> VocabularyEntry {
        entry(
            "Striper",
            &["striped", "stripper"],
            &[
                "opened",
                "open",
                "launch",
                "app",
                "application",
                "dictation",
            ],
        )
    }

    // PLAN.md §0: the app must learn its own name without breaking the
    // ordinary words it is misheard as.
    #[test]
    fn striper_is_corrected_in_context() {
        let e = [striper()];
        assert_eq!(apply("I opened striped.", &e), "I opened Striper.");
        assert_eq!(
            apply("Please launch stripper now", &e),
            "Please launch Striper now"
        );
        assert_eq!(
            apply("The striped app is great", &e),
            "The Striper app is great"
        );
    }

    #[test]
    fn striper_does_not_rewrite_ordinary_uses() {
        let e = [striper()];
        assert_eq!(
            apply("He wore a striped shirt.", &e),
            "He wore a striped shirt."
        );
        assert_eq!(
            apply("The paint stripper removed the varnish.", &e),
            "The paint stripper removed the varnish."
        );
        assert_eq!(
            apply("Striped bass are biting.", &e),
            "Striped bass are biting."
        );
    }

    #[test]
    fn capitalized_mid_sentence_alias_is_a_name_signal() {
        let e = [entry("Striper", &["striped"], &[])];
        assert_eq!(
            apply("I really like Striped for this.", &e),
            "I really like Striper for this."
        );
        // Sentence-initial capitals carry no signal.
        assert_eq!(
            apply("Striped fabric is nice.", &e),
            "Striped fabric is nice."
        );
    }

    #[test]
    fn capitalized_aliases_are_not_blindly_replaced() {
        // Another person called Steven must survive (PLAN.md M2 demo).
        let e = [entry("Stephen", &["Steven"], &["smith"])];
        assert_eq!(apply("Steven Smith called.", &e), "Stephen Smith called.");
        assert_eq!(
            apply("Then Steven Jones called.", &e),
            "Then Steven Jones called."
        );
    }

    #[test]
    fn always_policy_replaces_everywhere_but_respects_negative_context() {
        let mut e = entry("Kubernetes", &["cooper netties"], &[]);
        e.policy = MatchPolicy::Always;
        e.negative_context_words = vec!["sheldon".into()];
        assert_eq!(
            apply("deploy to cooper netties.", &[e.clone()]),
            "deploy to Kubernetes."
        );
        assert_eq!(
            apply("Sheldon Cooper netties", &[e]),
            "Sheldon Cooper netties"
        );
    }

    #[test]
    fn disabled_or_unconfirmed_entries_do_nothing() {
        let mut e = striper();
        e.enabled = false;
        assert_eq!(apply("I opened striped", &[e.clone()]), "I opened striped");
        e.enabled = true;
        e.user_confirmed = false;
        assert_eq!(apply("I opened striped", &[e]), "I opened striped");
    }

    #[test]
    fn app_scope_limits_where_entries_apply() {
        let mut e = entry("Conner", &["Connor"], &[]);
        e.policy = MatchPolicy::Always;
        e.scope = Scope::App("outlook.exe".into());
        let entries = [e];
        let outlook = ApplyContext {
            app_id: Some("OUTLOOK.EXE"),
        };
        let notepad = ApplyContext {
            app_id: Some("notepad.exe"),
        };
        assert_eq!(
            apply_vocabulary("Hi Connor", &entries, &outlook).text,
            "Hi Conner"
        );
        assert_eq!(
            apply_vocabulary("Hi Connor", &entries, &notepad).text,
            "Hi Connor"
        );
        assert_eq!(
            apply_vocabulary("Hi Connor", &entries, &ApplyContext::default()).text,
            "Hi Connor"
        );
    }

    #[test]
    fn preserves_punctuation_and_all_caps() {
        let mut e = striper();
        e.policy = MatchPolicy::Always;
        assert_eq!(apply("OPEN STRIPED!", &[e.clone()]), "OPEN STRIPER!");
        assert_eq!(apply("(striped), done", &[e]), "(Striper), done");
    }

    #[test]
    fn multiword_aliases_and_longest_match_win() {
        let mut e = entry("Striper", &["stripe her", "stripe"], &[]);
        e.policy = MatchPolicy::Always;
        assert_eq!(apply("open stripe her now", &[e]), "open Striper now");
    }

    #[test]
    fn lowercase_preferred_spelling_is_case_fixed_only_in_context() {
        let e = [entry("Striper", &[], &["app"])];
        assert_eq!(apply("the striper app", &e), "the Striper app");
        assert_eq!(
            apply("a striper painted lines", &e),
            "a striper painted lines"
        );
    }

    #[test]
    fn already_correct_text_is_untouched() {
        let out = apply_vocabulary("I opened Striper.", &[striper()], &ApplyContext::default());
        assert!(out.applied.is_empty());
        assert_eq!(out.text, "I opened Striper.");
    }

    #[test]
    fn reports_applied_rules() {
        let out = apply_vocabulary("I opened striped.", &[striper()], &ApplyContext::default());
        assert_eq!(out.applied.len(), 1);
        assert_eq!(out.applied[0].original, "striped");
        assert_eq!(
            out.applied[0].reason,
            MatchReason::ContextWord("opened".into())
        );
    }

    #[test]
    fn unicode_text_is_handled() {
        let mut e = entry("Zoë", &["Zoe"], &[]);
        e.policy = MatchPolicy::Always;
        assert_eq!(apply("Café avec Zoe, merci", &[e]), "Café avec Zoë, merci");
    }

    #[test]
    fn learns_nearby_content_words() {
        let text = "I opened striped to dictate my email";
        let start = text.find("striped").unwrap();
        let words = learn_context_words(text, start..start + 7, &["striped", "Striper"]);
        assert_eq!(words, vec!["opened", "dictate", "email"]);
    }

    #[test]
    fn learning_skips_stopwords_and_the_word_itself() {
        let text = "my name is Steven";
        let start = text.find("Steven").unwrap();
        let words = learn_context_words(text, start..text.len(), &["Steven", "Stephen"]);
        assert!(words.is_empty());
    }
}
