//! Purpose: Expose built-in command adapters used by the proxy registry.
//! Caller: main module and proxy::run registry setup.
//! Dependencies: Individual adapter modules.
//! Main Functions: Module exports for built-in command adapters.
//! Side Effects: None.

pub mod build;
pub mod cloud;
pub mod common;
pub mod containers;
pub mod database;
pub mod files;
pub mod generic;
pub mod git;
pub mod lint;
pub mod logs;
pub mod search;
pub mod tests;
