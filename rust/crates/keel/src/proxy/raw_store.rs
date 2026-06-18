//! Purpose: Persist raw and compact command-output artifacts for proxy recovery.
//! Caller: proxy::run after the real command executes and after adapter compaction.
//! Dependencies: Claude home resolution, serde metadata, and filesystem writes.
//! Main Functions: RawStore::save, RawStore::save_compact, RawStore::generate_id.
//! Side Effects: Creates raw-output directories and writes stdout/stderr/metadata/compact logs.

use crate::runtime::resolve_claude_home;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

        fs::write(dir.join("stdout.log"), &run.stdout)?;
        fs::write(dir.join("stderr.log"), &run.stderr)?;
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
            let modified = fs::metadata(&entry.path)
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::now());
            if modified < cutoff {
                fs::remove_dir_all(&entry.path)?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

impl Default for RawStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{RawRun, RawStore, RunMeta};
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

        let _ = std::fs::remove_dir_all(root);
    }
}
