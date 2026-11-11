//! Probatio scores a test suite. A kill is an assertion failure, not a bill of health.

pub mod args;
pub mod ast_ts;
pub mod commands;
pub mod hash;
pub mod memory;
pub mod queue;
pub mod schema_cmd;
pub mod script_line;
pub mod coverage;
pub mod decide;
pub mod diffscope;
pub mod envelope;
pub mod generate;
pub mod golden;
pub mod jobs;
pub mod json;
pub mod ledger;
pub mod matrix_cmd;
pub mod mcp;
pub mod operators;
pub mod process;
pub mod report;
pub mod run_options;
pub mod runner;
pub mod sealed;
pub mod slots;
pub mod tally;
pub mod testcode;
pub mod verify;
pub mod worktree;

pub fn dispatch(args: &[String]) -> envelope::Envelope {
    commands::dispatch(args)
}
