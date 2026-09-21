//! Purpose: Single owner for cross-process advisory file-lock bounds and contention classification.
//! Caller: proxy::event_log, proxy::raw_store, runner::tool_timings, utility::record_store lock helpers.
//! Main Functions: LOCK_TIMEOUT, LOCK_RETRY_INTERVAL, is_lock_contention.
//! Side Effects: None. Only constants and a pure predicate.

use std::time::Duration;

/// Bounded wait for a contended lock file: a stalled holder must never delay the caller indefinitely.
pub(crate) const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
/// Poll interval while waiting on a contended lock file.
pub(crate) const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(25);

/// True for lock contention: Unix WouldBlock or Windows sharing/lock OS codes 32/33.
pub(crate) fn is_lock_contention(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || error
            .raw_os_error()
            .is_some_and(|code| matches!(code, 32 | 33))
}
