//! Astra OS — Almanac command engine.
//!
//! Layers:
//! - [`lexer`] — whitespace/quote tokeniser that keeps `>` inside path tokens.
//! - [`parser`] — tokens into [`ast::AlmanacCommand`] (an unambiguous AST).
//! - [`engine`] — runs a command against [`crate::state::SystemState`],
//!   including the interactive-prompt state machine.
//! - [`completion`] — case-insensitive tab completion with lock protection.
//! - [`outcome`] — the structured, status-tagged result type.
//!
//! Anything that is not an `almanac …` line falls through to the controlled
//! host-shell layer in [`crate::shell`].

pub mod ast;
pub mod completion;
pub mod engine;
pub mod lexer;
pub mod outcome;
pub mod parser;

pub use completion::{complete, CompletionResult};
pub use engine::{cancel, evaluate, respond};
pub use outcome::{
    AlmanacOutcome, AppLaunch, OutputLine, ProcessView, PromptRequest, StatusTag, SystemAction,
};
pub use parser::parse_line;
