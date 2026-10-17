//! Hands-free dictation session (PLAN.md §5a): one keystroke or button
//! starts it, speech is committed at natural pauses, "mute"/"unmute" pause
//! and resume by voice, and the same keystroke ends it.
//!
//! This module is the pure decision layer. The app segments microphone audio
//! into utterances at pauses, transcribes them, and asks [`HandsFreeSession`]
//! what each one means. It never touches audio, the desktop or the LLM.

use serde::{Deserialize, Serialize};

/// Minimum silence on each side of a standalone "mute"/"unmute".
pub const STANDALONE_PAUSE_MS: u64 = 400;
/// While paused, longer utterances are discarded without being transcribed:
/// "unmute" never takes this long, and ordinary speech in the room should not
/// even reach the recognizer.
pub const PAUSED_MAX_UTTERANCE_MS: u64 = 2_000;

/// The three states the user sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Off,
    Listening,
    Paused,
}

/// Silence measured around one utterance by the audio segmenter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UtteranceTiming {
    /// Silence between the previous utterance (or session start/resume) and
    /// this one.
    pub silence_before_ms: u64,
    /// Silence after this utterance before it was cut.
    pub silence_after_ms: u64,
    /// Speech duration.
    pub duration_ms: u64,
}

impl UtteranceTiming {
    fn standalone(&self) -> bool {
        self.silence_before_ms >= STANDALONE_PAUSE_MS
            && self.silence_after_ms >= STANDALONE_PAUSE_MS
    }
}

/// What the app should do with one utterance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UtteranceAction {
    /// Insert this text (already formatted: "new paragraph" → breaks).
    Insert(String),
    /// Run a correction instruction (text after "correct that"; empty when
    /// the user paused after the wake phrase and the instruction follows).
    Correct(String),
    /// Enter Paused.
    Pause,
    /// Return to Listening.
    Resume,
    /// Throw the utterance away: never inserted, stored or sent anywhere.
    Discard,
}

/// What a state change means for feedback (indicator + sound).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    pub from: SessionState,
    pub to: SessionState,
}

#[derive(Debug, Clone)]
pub struct HandsFreeSession {
    state: SessionState,
    /// The user said "correct that" alone; the next utterance is the
    /// instruction.
    awaiting_instruction: bool,
    /// The tail of the text inserted so far in this run, to join the next
    /// utterance correctly (spacing, capitalization after a break).
    last_inserted: String,
}

impl Default for HandsFreeSession {
    fn default() -> Self {
        Self::new()
    }
}

impl HandsFreeSession {
    pub fn new() -> Self {
        Self {
            state: SessionState::Off,
            awaiting_instruction: false,
            last_inserted: String::new(),
        }
    }

    pub fn state(&self) -> SessionState {
        self.state
    }

    fn go(&mut self, to: SessionState) -> Option<Transition> {
        let from = self.state;
        if from == to {
            return None;
        }
        self.state = to;
        self.awaiting_instruction = false;
        if to != SessionState::Paused {
            // Off ends the run; Listening after Off starts a fresh one. A
            // resume keeps the join state so text continues naturally.
            if from == SessionState::Off || to == SessionState::Off {
                self.last_inserted.clear();
            }
        }
        Some(Transition { from, to })
    }

    /// Ctrl+Space / the big button: Off → Listening; Listening or Paused → Off.
    pub fn toggle(&mut self) -> Transition {
        let to = if self.state == SessionState::Off {
            SessionState::Listening
        } else {
            SessionState::Off
        };
        self.go(to).expect("toggle always changes state")
    }

    pub fn start(&mut self) -> Option<Transition> {
        self.go(SessionState::Listening)
    }

    pub fn end(&mut self) -> Option<Transition> {
        self.go(SessionState::Off)
    }

    pub fn pause(&mut self) -> Option<Transition> {
        (self.state == SessionState::Listening)
            .then(|| self.go(SessionState::Paused))
            .flatten()
    }

    pub fn resume(&mut self) -> Option<Transition> {
        (self.state == SessionState::Paused)
            .then(|| self.go(SessionState::Listening))
            .flatten()
    }

    /// The focused window changed or the user edited elsewhere: the next
    /// utterance must not assume it continues the previous one.
    pub fn reset_join(&mut self) {
        self.last_inserted.clear();
        self.awaiting_instruction = false;
    }

    /// Whether an utterance of this length should be transcribed at all.
    /// While paused, only short utterances can be "unmute".
    pub fn should_transcribe(&self, timing: &UtteranceTiming) -> bool {
        match self.state {
            SessionState::Off => false,
            SessionState::Listening => true,
            SessionState::Paused => timing.duration_ms <= PAUSED_MAX_UTTERANCE_MS,
        }
    }

    /// Decide what one transcribed utterance means, updating state for
    /// mute/unmute. Insert text is returned joined to what came before; call
    /// [`HandsFreeSession::inserted`] once it was actually inserted.
    pub fn on_utterance(&mut self, transcript: &str, timing: UtteranceTiming) -> UtteranceAction {
        match self.state {
            SessionState::Off => UtteranceAction::Discard,
            SessionState::Paused => {
                if timing.standalone() && is_command_word(transcript, UNMUTE) {
                    self.go(SessionState::Listening);
                    UtteranceAction::Resume
                } else {
                    UtteranceAction::Discard
                }
            }
            SessionState::Listening => {
                if self.awaiting_instruction {
                    self.awaiting_instruction = false;
                    let instruction = transcript.trim();
                    return if instruction.is_empty() {
                        UtteranceAction::Discard
                    } else {
                        UtteranceAction::Correct(instruction.to_string())
                    };
                }
                if timing.standalone() && is_command_word(transcript, MUTE) {
                    self.go(SessionState::Paused);
                    return UtteranceAction::Pause;
                }
                if let Some(rest) = correction_prefix(transcript) {
                    if rest.is_empty() {
                        self.awaiting_instruction = true;
                    }
                    return UtteranceAction::Correct(rest.to_string());
                }
                let formatted = apply_spoken_formatting(transcript);
                let joined = join_utterance(&self.last_inserted, &formatted);
                if joined.is_empty() {
                    UtteranceAction::Discard
                } else {
                    UtteranceAction::Insert(joined)
                }
            }
        }
    }

    /// Record text actually inserted, so the next utterance joins it.
    pub fn inserted(&mut self, text: &str) {
        self.last_inserted.push_str(text);
        // Only the tail matters for joining.
        if self.last_inserted.len() > 64 {
            let mut cut = self.last_inserted.len() - 64;
            while !self.last_inserted.is_char_boundary(cut) {
                cut += 1;
            }
            self.last_inserted.drain(..cut);
        }
    }
}

const MUTE: &[&str] = &["mute"];
const UNMUTE: &[&str] = &["unmute", "un mute", "un-mute"];

/// The whole utterance, ignoring case and surrounding punctuation, is one of
/// `words`. "I put my phone on mute" is not.
fn is_command_word(transcript: &str, words: &[&str]) -> bool {
    let cleaned: String = transcript
        .trim()
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase();
    words.contains(&cleaned.as_str())
}

/// "Correct that, …" at the very start of an utterance. Returns the rest.
pub fn correction_prefix(text: &str) -> Option<&str> {
    let text = text.trim();
    let mut words = text.splitn(3, char::is_whitespace);
    let first = words
        .next()?
        .trim_end_matches([',', '.', ';', ':', '!', '?']);
    if !first.eq_ignore_ascii_case("correct") {
        return None;
    }
    let second = words
        .next()?
        .trim_end_matches(['.', ',', ';', ':', '!', '?']);
    if !second.eq_ignore_ascii_case("that") {
        return None;
    }
    Some(
        words
            .next()
            .unwrap_or("")
            .trim_start_matches([' ', '.', ',', ';', ':'])
            .trim(),
    )
}

/// Spoken formatting commands Dragon users rely on.
const SPOKEN_BREAKS: &[(&str, &str)] = &[("new paragraph", "\n\n"), ("new line", "\n")];

/// Turn spoken "new paragraph" / "new line" into breaks. Punctuation the
/// recognizer attached to the command ("…tomorrow. New paragraph. Dear…")
/// is absorbed, sentence-ending punctuation before it is kept, and the word
/// after a break is capitalized.
pub fn apply_spoken_formatting(text: &str) -> String {
    let lower = text.to_lowercase();
    // Lowercasing can change byte lengths for some scripts; only use the
    // fast path when offsets line up.
    if lower.len() != text.len() {
        return text.trim().to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let bytes = text.as_bytes();
    'scan: while i < text.len() {
        for (phrase, brk) in SPOKEN_BREAKS {
            if lower[i..].starts_with(phrase)
                && word_boundary_before(text, i)
                && word_boundary_after(text, i + phrase.len())
            {
                // Drop separators (spaces, commas) the recognizer put before
                // the command, but keep sentence punctuation.
                let trimmed = out.trim_end_matches([' ', ',', ';', ':']).len();
                out.truncate(trimmed);
                // Collapse with a break that is already there.
                let existing = out.len() - out.trim_end_matches('\n').len();
                let wanted = brk.len();
                if existing < wanted {
                    out.push_str(&brk[existing..]);
                }
                i += phrase.len();
                // Skip punctuation and spaces the recognizer attached after.
                while i < text.len() && matches!(bytes[i], b' ' | b'.' | b',' | b';' | b':') {
                    i += 1;
                }
                continue 'scan;
            }
        }
        let ch = text[i..].chars().next().expect("in bounds");
        if out.ends_with('\n') && ch.is_alphabetic() {
            out.extend(ch.to_uppercase());
        } else {
            out.push(ch);
        }
        i += ch.len_utf8();
    }
    // A leading break is meaningful (the user said only "new paragraph");
    // trim spaces but not newlines at the ends.
    out.trim_matches(' ').to_string()
}

fn word_boundary_before(text: &str, i: usize) -> bool {
    text[..i]
        .chars()
        .last()
        .is_none_or(|c| !c.is_alphanumeric())
}

fn word_boundary_after(text: &str, i: usize) -> bool {
    text[i..]
        .chars()
        .next()
        .is_none_or(|c| !c.is_alphanumeric())
}

/// Join a new utterance onto text inserted earlier in the run: one space
/// between words, none after a line break, and a capital letter after a
/// paragraph or line break.
pub fn join_utterance(previous: &str, next: &str) -> String {
    if next.is_empty() {
        return String::new();
    }
    if previous.is_empty() || next.starts_with('\n') {
        return next.to_string();
    }
    if previous.ends_with('\n') {
        let mut chars = next.chars();
        let first = chars.next().expect("non-empty");
        return first.to_uppercase().chain(chars).collect();
    }
    if previous.ends_with(char::is_whitespace) || next.starts_with(char::is_whitespace) {
        return next.to_string();
    }
    let starts_with_punct = next
        .chars()
        .next()
        .is_some_and(|c| matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | ')'));
    if starts_with_punct {
        next.to_string()
    } else {
        format!(" {next}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paused_gap() -> UtteranceTiming {
        UtteranceTiming {
            silence_before_ms: 800,
            silence_after_ms: 800,
            duration_ms: 500,
        }
    }

    fn quick() -> UtteranceTiming {
        UtteranceTiming {
            silence_before_ms: 100,
            silence_after_ms: 800,
            duration_ms: 500,
        }
    }

    fn listening() -> HandsFreeSession {
        let mut s = HandsFreeSession::new();
        s.toggle();
        s
    }

    #[test]
    fn toggle_moves_between_off_and_listening_and_ends_from_paused() {
        let mut s = HandsFreeSession::new();
        assert_eq!(s.state(), SessionState::Off);
        let t = s.toggle();
        assert_eq!((t.from, t.to), (SessionState::Off, SessionState::Listening));
        assert_eq!(
            s.on_utterance("Mute.", paused_gap()),
            UtteranceAction::Pause
        );
        assert_eq!(s.state(), SessionState::Paused);
        let t = s.toggle();
        assert_eq!((t.from, t.to), (SessionState::Paused, SessionState::Off));
    }

    #[test]
    fn standalone_mute_pauses_and_unmute_resumes() {
        let mut s = listening();
        assert_eq!(
            s.on_utterance("Mute.", paused_gap()),
            UtteranceAction::Pause
        );
        assert_eq!(
            s.on_utterance("Unmute!", paused_gap()),
            UtteranceAction::Resume
        );
        assert_eq!(s.state(), SessionState::Listening);
        assert_eq!(s.on_utterance("mute", paused_gap()), UtteranceAction::Pause);
        assert_eq!(
            s.on_utterance("Un-mute.", paused_gap()),
            UtteranceAction::Resume
        );
    }

    #[test]
    fn mute_inside_a_sentence_is_dictation() {
        let mut s = listening();
        assert_eq!(
            s.on_utterance("I put my phone on mute.", paused_gap()),
            UtteranceAction::Insert("I put my phone on mute.".into())
        );
        assert_eq!(s.state(), SessionState::Listening);
    }

    #[test]
    fn mute_without_a_pause_before_it_is_dictation() {
        let mut s = listening();
        assert_eq!(
            s.on_utterance("Mute.", quick()),
            UtteranceAction::Insert("Mute.".into())
        );
        assert_eq!(s.state(), SessionState::Listening);
    }

    #[test]
    fn everything_but_unmute_is_discarded_while_paused() {
        let mut s = listening();
        s.on_utterance("Mute", paused_gap());
        for said in [
            "Hey, can you pass the salt?",
            "Correct that, change Thursday to Friday.",
            "New paragraph",
            "Please unmute the TV",
            "Mute.",
        ] {
            assert_eq!(
                s.on_utterance(said, paused_gap()),
                UtteranceAction::Discard,
                "{said}"
            );
            assert_eq!(s.state(), SessionState::Paused);
        }
        // "unmute" needs its pauses too.
        assert_eq!(s.on_utterance("Unmute.", quick()), UtteranceAction::Discard);
    }

    #[test]
    fn long_utterances_are_not_even_transcribed_while_paused() {
        let mut s = listening();
        assert!(s.should_transcribe(&UtteranceTiming {
            duration_ms: 20_000,
            ..paused_gap()
        }));
        s.pause();
        assert!(!s.should_transcribe(&UtteranceTiming {
            duration_ms: 2_500,
            ..paused_gap()
        }));
        assert!(s.should_transcribe(&paused_gap()));
        s.end();
        assert!(!s.should_transcribe(&paused_gap()));
    }

    #[test]
    fn off_discards_everything() {
        let mut s = HandsFreeSession::new();
        assert_eq!(
            s.on_utterance("Unmute", paused_gap()),
            UtteranceAction::Discard
        );
        assert_eq!(
            s.on_utterance("Hello there.", paused_gap()),
            UtteranceAction::Discard
        );
    }

    #[test]
    fn correct_that_routes_to_correction_in_one_breath_or_two() {
        let mut s = listening();
        assert_eq!(
            s.on_utterance("Correct that, change Thursday to Friday.", quick()),
            UtteranceAction::Correct("change Thursday to Friday.".into())
        );
        assert_eq!(
            s.on_utterance("Correct that.", paused_gap()),
            UtteranceAction::Correct(String::new())
        );
        assert_eq!(
            s.on_utterance("Start a new paragraph after see you tomorrow.", quick()),
            UtteranceAction::Correct("Start a new paragraph after see you tomorrow.".into())
        );
        // Back to dictation afterwards.
        assert!(matches!(
            s.on_utterance("Thanks again.", quick()),
            UtteranceAction::Insert(_)
        ));
    }

    #[test]
    fn correct_that_mid_sentence_is_dictation() {
        let mut s = listening();
        assert_eq!(
            s.on_utterance("Please correct that invoice.", quick()),
            UtteranceAction::Insert("Please correct that invoice.".into())
        );
    }

    #[test]
    fn new_paragraph_becomes_a_break() {
        assert_eq!(
            apply_spoken_formatting("See you tomorrow. New paragraph. dear John, thanks."),
            "See you tomorrow.\n\nDear John, thanks."
        );
        assert_eq!(
            apply_spoken_formatting("see you tomorrow new paragraph thanks"),
            "see you tomorrow\n\nThanks"
        );
        assert_eq!(apply_spoken_formatting("New paragraph."), "\n\n");
        assert_eq!(
            apply_spoken_formatting("First line, new line, second line."),
            "First line\nSecond line."
        );
        // "new paragraphs" or "renewed" don't match.
        assert_eq!(
            apply_spoken_formatting("We wrote new paragraphs."),
            "We wrote new paragraphs."
        );
        assert_eq!(apply_spoken_formatting("The renew line"), "The renew line");
    }

    #[test]
    fn repeated_breaks_do_not_stack_beyond_a_paragraph() {
        assert_eq!(
            apply_spoken_formatting("End. New paragraph new paragraph start"),
            "End.\n\nStart"
        );
    }

    #[test]
    fn utterances_join_with_spaces_and_capitals_after_breaks() {
        let mut s = listening();
        let first = s.on_utterance("Dear John,", quick());
        assert_eq!(first, UtteranceAction::Insert("Dear John,".into()));
        s.inserted("Dear John,");
        let UtteranceAction::Insert(second) = s.on_utterance("thanks for the letter.", quick())
        else {
            panic!()
        };
        assert_eq!(second, " thanks for the letter.");
        s.inserted(&second);
        let UtteranceAction::Insert(brk) = s.on_utterance("New paragraph.", paused_gap()) else {
            panic!()
        };
        assert_eq!(brk, "\n\n");
        s.inserted(&brk);
        let UtteranceAction::Insert(next) = s.on_utterance("see you soon.", quick()) else {
            panic!()
        };
        assert_eq!(next, "See you soon.");
    }

    #[test]
    fn resume_continues_the_run_but_restart_does_not() {
        let mut s = listening();
        s.inserted("Hello.");
        s.on_utterance("Mute", paused_gap());
        s.on_utterance("Unmute", paused_gap());
        assert_eq!(
            s.on_utterance("Again.", quick()),
            UtteranceAction::Insert(" Again.".into())
        );
        s.toggle();
        s.toggle();
        assert_eq!(
            s.on_utterance("Again.", quick()),
            UtteranceAction::Insert("Again.".into())
        );
    }

    #[test]
    fn punctuation_joins_without_a_space() {
        assert_eq!(join_utterance("Hello", ", world"), ", world");
        assert_eq!(join_utterance("Hello", "world"), " world");
        assert_eq!(join_utterance("Hello ", "world"), "world");
        assert_eq!(join_utterance("", "world"), "world");
    }
}
