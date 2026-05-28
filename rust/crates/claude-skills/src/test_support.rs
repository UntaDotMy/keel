//! Purpose: Shared synchronization primitives for the crate's test suite.
//! Caller: `#[cfg(test)] mod tests` blocks across the crate.
//! Dependencies: std::sync::Mutex.
//! Main Functions: ENV_LOCK accessor.
//! Side Effects: None at module scope; the Mutex is acquired per-test.
//!
//! Design note: Several modules under `claude-skills` mutate
//! process-global environment variables during their tests
//! (`CLAUDE_TARGET_OVERRIDE`, `CLAUDE_EFFORT`, `CLAUDE_PROJECT_DIR`,
//! `CLAUDE_SKILLS_HOOK`). Cargo runs tests across modules on a shared
//! thread pool, so a per-module Mutex only serializes within its own
//! module — two threads from different modules can still flip the same
//! env var concurrently and each see the other's value through
//! `resolve_claude_home`. The symptom we hit is a `tool_timings` test
//! that wrote its JSONL row, observed a `recall` test overwrite the
//! override, then tried to read the log back from its own (now stale)
//! directory and got `NotFound`.
//!
//! The fix is a single process-wide `ENV_LOCK` that every test which
//! mutates env vars takes before it touches them. Tests that do not
//! touch env vars do not need to hold it.

#![cfg(test)]

use std::sync::Mutex;

/// Process-wide guard around test-time mutation of environment
/// variables. Take this lock at the top of any test that calls
/// `std::env::set_var` or `std::env::remove_var`. Hold it for the
/// entire test body, including the read-back assertions that depend on
/// the env value still pointing at the test's private state.
///
/// Use `lock().unwrap_or_else(|poisoned| poisoned.into_inner())` so a
/// poisoned mutex from a panicking peer test does not cascade into a
/// second failure that masks the original. The poisoned-mutex case is
/// benign here — the next acquirer immediately overwrites the env var
/// with its own fresh value.
pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());
