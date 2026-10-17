//! Striper's platform-independent core (PLAN.md §8 `editor-core`).
//!
//! - [`vocabulary`]: learned spellings applied before insertion (Lane A).
//! - [`command`]: the closed set of edit operations and LLM response validation.
//! - [`parser`]: deterministic parsing of common spoken corrections.
//! - [`edit`]: resolving operations to text and verified keystroke execution.
//! - [`session`]: the dictation ledger and conversational correction session.
//! - [`store`]: SQLite persistence for vocabulary, audit and edit history.
//! - [`prompt`]: the bounded context sent to a command-interpreting LLM.

pub mod command;
pub mod edit;
pub mod parser;
pub mod prompt;
pub mod session;
pub mod store;
pub mod text;
pub mod vocabulary;
