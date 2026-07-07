//! Library facade for the keel runtime — re-exports the public Application
//! entry point so the binary crate stays thin.

// The `json!` macro builds the large MCP `handle_tools_list` literal; 512 is
// the conventional recursion-limit raise for sizeable serde_json constructions.
#![recursion_limit = "512"]

pub mod adapters;
mod args;
mod commands;
mod comment_lint;
pub mod error;
mod help;
mod hooks;
mod json;
mod manager;
mod mcp;
pub mod proxy;
mod review;
mod runner;
mod runtime;
mod slop_detector;
#[cfg(test)]
mod test_support;
mod utility;

pub use commands::Application;
