//! Purpose: Single owner for cross-process advisory file-lock bounds, contention classification, and the bounded exclusive-lock guard.
//! Caller: proxy::event_log, proxy::raw_store, runner::tool_timings, utility::record_store, utility::skill_match ledgers.
//! Main Functions: LOCK_TIMEOUT, LOCK_RETRY_INTERVAL, is_lock_contention, lock_exclusive.
//! Side Effects: lock_exclusive creates and holds the lock file until the returned guard drops.

use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use fs2::FileExt;

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

/// An exclusive advisory lock, released when this guard drops.
pub(crate) struct ExclusiveLock {
    file: fs::File,
}

impl Drop for ExclusiveLock {
    fn drop(&mut self) {
        // Qualify the trait method: rustc 1.89 added an inherent `File::unlock`
        // that would shadow this and break the declared 1.80 MSRV.
        let _ = fs2::FileExt::unlock(&self.file); // why: best-effort unlock on drop
    }
}

/// Take an exclusive advisory lock at `<directory>/<lock_name>`, waiting up to
/// [`LOCK_TIMEOUT`]. Hold the returned guard across a read-modify-write so
/// concurrent keel processes cannot lose each other's records.
pub(crate) fn lock_exclusive(directory: &Path, lock_name: &str) -> io::Result<ExclusiveLock> {
    fs::create_dir_all(directory)?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(lock_name))?;
    let deadline = Instant::now() + LOCK_TIMEOUT;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(ExclusiveLock { file }),
            Err(error) if is_lock_contention(&error) => {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "lock remained held past the bounded wait",
                    ));
                }
                std::thread::sleep(LOCK_RETRY_INTERVAL);
            }
            Err(error) => return Err(error),
        }
    }
}
