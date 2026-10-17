//! Striper's platform-independent core (PLAN.md §8 `editor-core`).
//!
//! - [`vocabulary`]: learned spellings applied before insertion (Lane A).
//! - [`command`]: the closed set of edit operations and LLM response validation.
//! - [`parser`]: deterministic parsing of common spoken corrections.
//! - [`dictation`]: the hands-free session (Listening / Paused / Off, mute, new paragraph).
//! - [`edit`]: resolving operations to text and verified keystroke execution.
//! - [`live`]: corrections against the focused field's live text (read, verify, apply).
//! - [`session`]: the dictation ledger and conversational correction session.
//! - [`store`]: SQLite persistence for vocabulary, audit and edit history.
//! - [`prompt`]: the bounded context sent to a command-interpreting LLM.

pub mod command;
pub mod dictation;
pub mod edit;
pub mod live;
pub mod migration;
pub mod parser;
pub mod prompt;
pub mod session;
pub mod store;
pub mod text;
pub mod vocabulary;
