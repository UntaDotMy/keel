//! Purpose: Entrypoint for the native claude-skills Rust binary.
//! Caller: Operating system process launch via cargo build artifact or installed binary.
//! Dependencies: args, commands, help, json, manager, review, runner, runtime, utility modules and Rust claude-skills crates.
//! Main Functions: main.
//! Side Effects: Reads CLI arguments, dispatches to command handlers, and writes to stdout/stderr.

use std::io::{self, Write};
use std::process::ExitCode;

pub mod adapters;
mod args;
mod commands;
mod help;
mod hooks;
mod json;
mod manager;
mod mcp;
pub mod proxy;
mod review;
mod runner;
mod runtime;
mod utility;

pub use commands::Application;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let build_version = option_env!("CLAUDE_SKILLS_BUILD_VERSION").unwrap_or("");

    let stdout_handle = io::stdout();
    let stderr_handle = io::stderr();
    let mut stdout_lock = stdout_handle.lock();
    let mut stderr_lock = stderr_handle.lock();

    let exit_code = Application::new(build_version).run(
        &arguments,
        &mut stdout_lock as &mut dyn Write,
        &mut stderr_lock as &mut dyn Write,
    );

    let _ = stdout_lock.flush();
    let _ = stderr_lock.flush();

    ExitCode::from(exit_code)
}
