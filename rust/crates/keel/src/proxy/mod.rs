//! Purpose: Provide the token-saving proxy layer separate from workflow and review commands.
//! Caller: runner::run_run_command and command adapters.
//! Dependencies: adapter contracts, command classification, raw storage, rendering, and token metering.
//! Main Functions: run_proxy plus shared proxy data types.
//! Side Effects: Submodules may execute commands and write raw-output recovery artifacts.

pub mod adapter;
pub mod adapters;
pub mod classify;
pub mod command_ast;
pub mod event_log;
pub mod filters;
pub mod injection_guard;
pub mod raw_store;
pub mod registry;
pub mod render;
pub mod run;
pub mod token_meter;

pub use adapter::{CommandAdapter, CompactResult};
pub use command_ast::{CommandAst, CommandKind};
pub use raw_store::{RawRun, RunMeta};
pub use registry::AdapterRegistry;
pub use run::run_proxy;
