//! Unicode-aware text helpers shared by the vocabulary engine, the command
//! parser and the edit planner.
//!
//! Caret movement in desktop text fields is (almost everywhere) by grapheme
//! cluster, and `\r\n` counts as a single caret step, so every length that
//! becomes a keystroke count is measured in graphemes here.

use unicode_segmentation::UnicodeSegmentation;

/// Number of caret positions a string occupies.
pub fn grapheme_len(s: &str) -> usize {
    s.graphemes(true).count()
}

/// A word token with its byte span in the source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token<'a> {
    pub text: &'a str,
    pub start: usize,
    pub end: usize,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
}

fn is_inner_joiner(c: char) -> bool {
    matches!(c, '\'' | '’' | '-')
}

/// Split text into word tokens. Apostrophes and hyphens are kept inside a
/// word ("don't", "Jean-Luc") but never at its edges.
pub fn tokenize(text: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (start, c) = chars[i];
        if !is_word_char(c) {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < chars.len() {
            let (_, cj) = chars[j];
            if is_word_char(cj) {
                j += 1;
            } else if is_inner_joiner(cj) && j + 1 < chars.len() && is_word_char(chars[j + 1].1) {
                j += 2;
            } else {
                break;
            }
        }
        let end = if j < chars.len() {
            chars[j].0
        } else {
            text.len()
        };
        tokens.push(Token {
            text: &text[start..end],
            start,
            end,
        });
        i = j;
    }
    tokens
}

/// Case-fold for comparisons (lowercase + typographic apostrophe folding).
pub fn fold(s: &str) -> String {
    s.to_lowercase().replace('’', "'")
}

/// True when the first cased character is uppercase and some later cased
/// character is lowercase ("Striper", not "STRIPER" or "striper").
pub fn is_capitalized(s: &str) -> bool {
    let letters: Vec<char> = s.chars().filter(|c| c.is_alphabetic()).collect();
    match letters.first() {
        Some(first) if first.is_uppercase() => {
            letters.len() == 1 || letters[1..].iter().any(|c| c.is_lowercase())
        }
        _ => false,
    }
}

/// True when every cased character is uppercase and there are at least two.
pub fn is_all_caps(s: &str) -> bool {
    let letters: Vec<char> = s.chars().filter(|c| c.is_alphabetic()).collect();
    letters.len() >= 2 && letters.iter().all(|c| c.is_uppercase())
}

/// Uppercase the first character.
pub fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Whether the token starting at `byte_start` opens a sentence: it is the
/// first word, or the previous non-space character ends a sentence.
pub fn opens_sentence(text: &str, byte_start: usize) -> bool {
    match text[..byte_start].trim_end().chars().last() {
        None => true,
        Some(c) => matches!(c, '.' | '!' | '?' | '\n' | '…'),
    }
}

/// Re-case `replacement` to follow how the user's text was cased, while
/// keeping deliberate interior capitals of the preferred spelling
/// ("McDonald", "iPhone").
pub fn match_case(original: &str, replacement: &str, sentence_start: bool) -> String {
    if is_all_caps(original) {
        return replacement.to_uppercase();
    }
    let replacement_has_capital = replacement.chars().any(|c| c.is_uppercase());
    if replacement_has_capital {
        // A preferred spelling with capitals is a proper spelling; keep it.
        return replacement.to_string();
    }
    if sentence_start && is_capitalized(original) {
        return capitalize_first(replacement);
    }
    replacement.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grapheme_len_counts_caret_steps() {
        assert_eq!(grapheme_len("abc"), 3);
        assert_eq!(grapheme_len("e\u{301}"), 1);
        assert_eq!(grapheme_len("a\r\nb"), 3);
        assert_eq!(grapheme_len("👍🏽"), 1);
        assert_eq!(grapheme_len(""), 0);
    }

    #[test]
    fn tokenize_keeps_inner_apostrophes_and_hyphens() {
        let toks: Vec<&str> = tokenize("I'll see Jean-Luc on Thursday.")
            .iter()
            .map(|t| t.text)
            .collect();
        assert_eq!(toks, vec!["I'll", "see", "Jean-Luc", "on", "Thursday"]);
    }

    #[test]
    fn tokenize_drops_edge_punctuation() {
        let toks: Vec<&str> = tokenize("'quoted' -dash- end.")
            .iter()
            .map(|t| t.text)
            .collect();
        assert_eq!(toks, vec!["quoted", "dash", "end"]);
    }

    #[test]
    fn tokenize_reports_byte_spans_for_unicode() {
        let text = "Café Zoë";
        let toks = tokenize(text);
        assert_eq!(&text[toks[1].start..toks[1].end], "Zoë");
    }

    #[test]
    fn case_matching() {
        assert_eq!(match_case("STRIPED", "Striper", false), "STRIPER");
        assert_eq!(match_case("striped", "Striper", false), "Striper");
        assert_eq!(match_case("Colour", "color", true), "Color");
        assert_eq!(match_case("colour", "color", false), "color");
        assert_eq!(match_case("iphone", "iPhone", false), "iPhone");
    }

    #[test]
    fn sentence_start_detection() {
        let t = "Hello there. Striped shirts";
        assert!(opens_sentence(t, 0));
        assert!(opens_sentence(t, t.find("Striped").unwrap()));
        assert!(!opens_sentence(t, t.find("there").unwrap()));
    }
}
