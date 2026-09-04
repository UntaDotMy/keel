//! Purpose: Open Keel's SQLite stores with platform-correct filesystem semantics.
//! Caller: recall and workspace-index persistence owners.
//! Dependencies: rusqlite and std::path.
//! Main Functions: open_connection.
//! Side Effects: Opens or creates the requested SQLite database.

use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
use std::time::Duration;

use rusqlite::Connection;
#[cfg(windows)]
use rusqlite::OpenFlags;

pub(crate) fn open_connection(path: &Path) -> rusqlite::Result<Connection> {
    let connection = {
        #[cfg(windows)]
        {
            Connection::open_with_flags_and_vfs(
                windows_extended_path(path),
                OpenFlags::default(),
                "win32-longpath",
            )
        }
        #[cfg(not(windows))]
        {
            Connection::open(path)
        }
    }?;
    connection.busy_timeout(Duration::from_secs(10))?;
    Ok(connection)
}

pub(crate) fn create_parent_directory(path: &Path) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    #[cfg(windows)]
    {
        std::fs::create_dir_all(windows_extended_path(parent))
    }
    #[cfg(not(windows))]
    {
        std::fs::create_dir_all(parent)
    }
}

#[cfg(windows)]
fn windows_extended_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let raw = absolute.to_string_lossy().replace('/', "\\");
    if raw.starts_with(r"\\?\") || raw.starts_with(r"\\.\") {
        return PathBuf::from(raw);
    }
    if let Some(unc) = raw.strip_prefix(r"\\") {
        return PathBuf::from(format!(r"\\?\UNC\{unc}"));
    }
    PathBuf::from(format!(r"\\?\{raw}"))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn opens_database_beyond_legacy_windows_path_limit() {
        let root = crate::test_support::unique_temp_dir("keel-sqlite-longpath");
        let mut directory = root.to_path_buf();
        while directory.to_string_lossy().encode_utf16().count() < 300 {
            directory.push("segment-xxxxxxxxxxxxxxxxxxxxxxxx");
        }
        let database = directory.join("index.sqlite3");

        create_parent_directory(&database).expect("create long database parent");
        let connection = open_connection(&database).expect("open long-path database");
        connection
            .execute_batch("CREATE TABLE proof(value TEXT); INSERT INTO proof VALUES('ok');")
            .expect("write long-path database");
        drop(connection);
        assert!(database.is_file());
    }
}
