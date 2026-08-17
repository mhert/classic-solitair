//! `soltool` — asset extraction and theme authoring CLI.
//!
//! Thin binary: parse the CLI, call the library, map the outcome to an
//! exit code, print errors to stderr. See `soltool::run`'s doc for the
//! exit-code contract. All actual logic lives in the library (`src/lib.rs`
//! and its modules), which is what the integration tests under `tests/`
//! exercise by spawning this binary.

use clap::Parser as _;
use soltool::{Cli, ExtractError, ToolError};

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match soltool::run(cli.command) {
        Ok(message) => {
            println!("{message}");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            let code = match &error {
                ToolError::Extract(ExtractError::AnimateRequiresResourceInput) => 2,
                _ => 1,
            };
            std::process::ExitCode::from(code)
        }
    }
}
