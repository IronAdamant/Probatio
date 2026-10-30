//! Probatio scores a test suite. A kill is an assertion failure, not a bill of health.

pub mod args;
pub mod commands;
pub mod decide;
pub mod envelope;
pub mod generate;
pub mod json;
pub mod mcp;
pub mod operators;
pub mod process;
pub mod runner;
pub mod worktree;

pub fn dispatch(args: &[String]) -> envelope::Envelope {
    commands::dispatch(args)
}
