//! Purpose: Persist raw and compact command-output artifacts for proxy recovery.
//! Caller: proxy::run after the real command executes and after adapter compaction.
//! Dependencies: harness home resolution, serde metadata, and filesystem writes.
//! Main Functions: RawStore::save, RawStore::save_compact, RawStore::generate_id.
//! Side Effects: Creates raw-output directories and writes stdout/stderr/metadata/compact logs.

use crate::runtime::resolve_claude_home;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Defense-in-depth disk cap. The capture chokepoint (runtime::run_command and
/// the streaming path) already caps at MAX_CAPTURED_OUTPUT_BYTES, but a future
/// caller could construct a RawRun directly — this ensures save() never writes
/// an unbounded stream to disk. Matches the capture cap.
const MAX_RAW_WRITE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunMeta {
    pub raw_id: String,
    pub command: String,
    #[serde(default)]
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub started_at: u64,
    pub duration_ms: u64,
    pub exit_code: i32,
    pub adapter_name: String,
    pub raw_path: PathBuf,
    pub compact_path: PathBuf,
    pub agent: String,
    pub workspace: PathBuf,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub compact_stdout_bytes: usize,
    pub compact_stderr_bytes: usize,
    pub estimated_tokens_before: usize,
    pub estimated_tokens_after: usize,
    pub estimated_tokens_saved: isize,
    pub savings_pct: f64,
    pub compacted: bool,
}

#[derive(Debug, Clone)]
pub struct RawRun {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}

pub struct RawStore {
    root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RawEntry {
    pub raw_id: String,
    pub path: PathBuf,
    pub meta: Option<RunMeta>,
}

impl RawStore {
    pub fn new() -> Self {
        let root = resolve_claude_home("")
            .map(|p| p.join("raw-output"))
            .unwrap_or_else(|_| std::env::temp_dir().join("keel-raw-output"));
        Self { root }
    }

    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    pub fn save(&self, meta: &mut RunMeta, run: &RawRun) -> std::io::Result<()> {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let dir = self.root.join(date).join(&meta.raw_id);
        fs::create_dir_all(&dir)?;

        // Defense-in-depth: never write an unbounded stream to disk. The capture
        // chokepoint already caps, but a direct RawRun caller could bypass it.
        let stdout_bytes = if run.stdout.len() > MAX_RAW_WRITE_BYTES {
            &run.stdout[..MAX_RAW_WRITE_BYTES]
        } else {
            &run.stdout[..]
        };
        let stderr_bytes = if run.stderr.len() > MAX_RAW_WRITE_BYTES {
            &run.stderr[..MAX_RAW_WRITE_BYTES]
        } else {
            &run.stderr[..]
        };
        fs::write(dir.join("stdout.log"), stdout_bytes)?;
        fs::write(dir.join("stderr.log"), stderr_bytes)?;
        fs::write(dir.join("command.txt"), &meta.command)?;

        let meta_json = serde_json::to_string_pretty(meta)?;
        fs::write(dir.join("meta.json"), meta_json)?;

        meta.raw_path = dir;
        Ok(())
    }

    pub fn save_compact(&self, meta: &RunMeta, compact_output: &str) -> std::io::Result<()> {
        if meta.raw_path.as_os_str().is_empty() {
            return Ok(());
        }
        fs::write(&meta.compact_path, compact_output)?;
        let meta_json = serde_json::to_string_pretty(meta)?;
        fs::write(meta.raw_path.join("meta.json"), meta_json)?;
        Ok(())
    }

    pub fn generate_id() -> String {
        let now = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let random: u32 = rand::random();
        format!("{now}-{:08x}", random)
    }

    pub fn find_dir(&self, raw_id: &str) -> io::Result<PathBuf> {
        let trimmed = raw_id.trim();
        if trimmed.is_empty()
            || trimmed.contains('/')
            || trimmed.contains('\\')
            || trimmed == "."
            || trimmed == ".."
            || trimmed.contains("..")
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid raw id",
            ));
        }
        for day in fs::read_dir(&self.root)? {
            let day = day?;
            if !day.file_type()?.is_dir() {
                continue;
            }
            let candidate = day.path().join(trimmed);
            if candidate.is_dir() {
                return Ok(candidate);
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("raw id not found: {trimmed}"),
        ))
    }

    pub fn load_meta(&self, raw_id: &str) -> io::Result<RunMeta> {
        let dir = self.find_dir(raw_id)?;
        let text = fs::read_to_string(dir.join("meta.json"))?;
        serde_json::from_str(&text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub fn read_file(&self, raw_id: &str, file_name: &str) -> io::Result<Vec<u8>> {
        let dir = self.find_dir(raw_id)?;
        fs::read(dir.join(file_name))
    }

    pub fn list(&self) -> io::Result<Vec<RawEntry>> {
        let mut entries = Vec::new();
        if !self.root.exists() {
            return Ok(entries);
        }
        for day in fs::read_dir(&self.root)? {
            let day = day?;
            if !day.file_type()?.is_dir() {
                continue;
            }
            for raw in fs::read_dir(day.path())? {
                let raw = raw?;
                if !raw.file_type()?.is_dir() {
                    continue;
                }
                let raw_id = raw.file_name().to_string_lossy().to_string();
                let meta = fs::read_to_string(raw.path().join("meta.json"))
                    .ok()
                    .and_then(|text| serde_json::from_str::<RunMeta>(&text).ok());
                entries.push(RawEntry {
                    raw_id,
                    path: raw.path(),
                    meta,
                });
            }
        }
        entries.sort_by(|left, right| right.raw_id.cmp(&left.raw_id));
        Ok(entries)
    }

    pub fn prune_older_than(&self, days: u64) -> io::Result<usize> {
        if !self.root.exists() {
            return Ok(0);
        }
        let cutoff = SystemTime::now()
            .checked_sub(Duration::from_secs(days.saturating_mul(86_400)))
            .unwrap_or(UNIX_EPOCH);
        let mut removed = 0usize;
        for entry in self.list()? {
            // why: date folders age by logical day even if mtime was touched.
            let age_signal = entry_age_signal(&entry.path).unwrap_or_else(|_| SystemTime::now());
            if age_signal < cutoff {
                fs::remove_dir_all(&entry.path)?;
                removed += 1;
            }
        }
        // why: remove empty YYYY-MM-DD shells left after entry prunes.
        if let Ok(days) = fs::read_dir(&self.root) {
            for day in days.flatten() {
                if !day.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                if fs::read_dir(day.path())
                    .map(|mut it| it.next().is_none())
                    .unwrap_or(false)
                {
                    let _ = fs::remove_dir(day.path()); // concurrent prune race ok
                }
            }
        }
        Ok(removed)
    }
}

/// Resolve an age signal for a raw entry path: parent `YYYY-MM-DD` folder midnight
/// UTC when parseable, otherwise the path's filesystem modified time.
fn entry_age_signal(path: &std::path::Path) -> io::Result<SystemTime> {
    if let Some(day) = path.parent().and_then(|p| p.file_name()) {
        let day = day.to_string_lossy();
        if let Some(ts) = parse_yyyy_mm_dd_midnight_utc(&day) {
            return Ok(ts);
        }
    }
    fs::metadata(path)?.modified()
}

fn parse_yyyy_mm_dd_midnight_utc(day: &str) -> Option<SystemTime> {
    // Strict `YYYY-MM-DD` only. Avoid treating raw ids as dates.
    if day.len() != 10
        || day.as_bytes().get(4) != Some(&b'-')
        || day.as_bytes().get(7) != Some(&b'-')
    {
        return None;
    }
    let year: i32 = day.get(0..4)?.parse().ok()?;
    let month: u32 = day.get(5..7)?.parse().ok()?;
    let day_n: u32 = day.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day_n) {
        return None;
    }
    // Approximate days since Unix epoch without pulling chrono into this path
    // (raw_store already depends on chrono for save(), but keep this pure).
    let y = year as i64;
    let m = month as i64;
    let d = day_n as i64;
    // Civil-from-days inverse (Howard Hinnant) → days since 1970-01-01.
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    if days < 0 {
        return Some(UNIX_EPOCH);
    }
    Some(UNIX_EPOCH + Duration::from_secs((days as u64).saturating_mul(86_400)))
}

impl Default for RawStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_yyyy_mm_dd_midnight_utc, RawRun, RawStore, RunMeta};
    use std::path::PathBuf;

    #[test]
    fn raw_store_saves_and_loads_metadata_and_streams() {
        let root = std::env::temp_dir().join(format!("keel-raw-store-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = RawStore::with_root(root.clone());
        let mut meta = RunMeta {
            raw_id: "20260512-143012-a1b2c3d4".to_string(),
            command: "pytest tests -q".to_string(),
            program: "pytest".to_string(),
            args: vec!["tests".to_string(), "-q".to_string()],
            cwd: PathBuf::from("."),
            started_at: 1,
            duration_ms: 2,
            exit_code: 1,
            adapter_name: "tests".to_string(),
            raw_path: PathBuf::new(),
            compact_path: PathBuf::new(),
            agent: "test".to_string(),
            workspace: PathBuf::from("."),
            stdout_bytes: 6,
            stderr_bytes: 5,
            compact_stdout_bytes: 0,
            compact_stderr_bytes: 0,
            estimated_tokens_before: 3,
            estimated_tokens_after: 0,
            estimated_tokens_saved: 0,
            savings_pct: 0.0,
            compacted: false,
        };
        let run = RawRun {
            stdout: b"stdout".to_vec(),
            stderr: b"error".to_vec(),
            exit_code: 1,
        };
        store.save(&mut meta, &run).expect("save");
        meta.compact_path = meta.raw_path.join("compact.txt");
        store.save_compact(&meta, "FAIL pytest").expect("compact");

        let loaded = store.load_meta(&meta.raw_id).expect("load meta");
        assert_eq!(loaded.command, "pytest tests -q");
        assert_eq!(
            store.read_file(&meta.raw_id, "stdout.log").expect("stdout"),
            b"stdout"
        );
        assert!(store.find_dir(&meta.raw_id).expect("dir").is_dir());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn raw_store_rejects_path_traversal_ids() {
        let root = std::env::temp_dir().join(format!(
            "keel-raw-store-traversal-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("2026-05-12")).expect("create day");
        let store = RawStore::with_root(root.clone());

        for raw_id in ["..", ".", "abc/def", r"abc\def", "abc..def"] {
            let error = store.find_dir(raw_id).expect_err("invalid raw id");
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        }

        let _ = std::fs::remove_dir_all(root); // best-effort test cleanup
    }

    #[test]
    fn prune_older_than_uses_date_folder_not_only_mtime() {
        let root = std::env::temp_dir().join(format!("keel-raw-prune-date-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root); // best-effort pre-clean
        let store = RawStore::with_root(root.clone());
        // Logical day far in the past; mtime is "now" after create_dir_all.
        let stale = root.join("2001-02-03").join("stale-id");
        std::fs::create_dir_all(&stale).expect("stale dir");
        std::fs::write(stale.join("stdout.log"), b"old").expect("stdout");
        let fresh = root.join("2099-01-01").join("fresh-id");
        std::fs::create_dir_all(&fresh).expect("fresh dir");
        std::fs::write(fresh.join("stdout.log"), b"new").expect("stdout");
        let removed = store.prune_older_than(30).expect("prune");
        assert_eq!(removed, 1, "only the 2001 day entry should prune");
        assert!(!stale.exists(), "stale entry removed");
        assert!(!root.join("2001-02-03").exists(), "empty day dir removed");
        assert!(fresh.exists(), "future-dated entry kept");
        let _ = std::fs::remove_dir_all(&root); // best-effort test cleanup
    }

    #[test]
    fn parse_yyyy_mm_dd_midnight_utc_rejects_garbage() {
        assert!(parse_yyyy_mm_dd_midnight_utc("not-a-date").is_none());
        assert!(parse_yyyy_mm_dd_midnight_utc("2026-13-01").is_none());
        assert!(parse_yyyy_mm_dd_midnight_utc("2026-07-16").is_some());
    }

    #[test]
    fn raw_store_caps_oversized_stdout_to_disk() {
        // H4 defense-in-depth: a RawRun with stdout over MAX_RAW_WRITE_BYTES
        // must not write the full stream to disk. The on-disk file is capped.
        let root =
            std::env::temp_dir().join(format!("keel-raw-store-cap-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = RawStore::with_root(root.clone());
        let mut meta = RunMeta {
            raw_id: "20260512-captest".to_string(),
            command: "runaway".to_string(),
            program: "runaway".to_string(),
            args: vec![],
            cwd: PathBuf::from("."),
            started_at: 1,
            duration_ms: 2,
            exit_code: 0,
            adapter_name: "generic".to_string(),
            raw_path: PathBuf::new(),
            compact_path: PathBuf::new(),
            agent: "test".to_string(),
            workspace: PathBuf::from("."),
            stdout_bytes: 0,
            stderr_bytes: 0,
            compact_stdout_bytes: 0,
            compact_stderr_bytes: 0,
            estimated_tokens_before: 0,
            estimated_tokens_after: 0,
            estimated_tokens_saved: 0,
            savings_pct: 0.0,
            compacted: false,
        };
        // Build a stdout vector 10 MiB over the cap.
        let over = super::MAX_RAW_WRITE_BYTES + (10 * 1024 * 1024);
        let run = RawRun {
            stdout: vec![b'x'; over],
            stderr: vec![],
            exit_code: 0,
        };
        store.save(&mut meta, &run).expect("save oversized run");

        let stdout_log = std::fs::read(meta.raw_path.join("stdout.log")).expect("read stdout.log");
        assert!(
            stdout_log.len() <= super::MAX_RAW_WRITE_BYTES,
            "on-disk stdout must be capped: {} > {}",
            stdout_log.len(),
            super::MAX_RAW_WRITE_BYTES
        );

        let _ = std::fs::remove_dir_all(root);
    }
}
