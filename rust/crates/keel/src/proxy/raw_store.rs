//! Purpose: Persist raw and compact command-output artifacts for proxy recovery.
//! Caller: proxy::run after the real command executes and after adapter compaction.
//! Dependencies: harness home resolution, serde metadata, and filesystem writes.
//! Main Functions: RawStore::save, RawStore::save_compact, RawStore::generate_id.
//! Side Effects: Creates raw-output directories and writes stdout/stderr/metadata/compact logs.

use crate::proxy::execution::ExecutionIdentity;
use crate::runtime::resolve_claude_home;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io;
#[cfg(unix)]
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Defense-in-depth disk cap. The capture chokepoint (runtime::run_command and
/// the streaming path) already caps at MAX_CAPTURED_OUTPUT_BYTES, but a future
/// caller could construct a RawRun directly — this ensures save() never writes
/// an unbounded stream to disk. Matches the capture cap.
const MAX_RAW_WRITE_BYTES: usize = 64 * 1024 * 1024;

/// Default raw-output retention when neither the plugin userConfig knob nor the
/// operator env var is set. Mirrors RAW_OUTPUT_DEFAULT_RETENTION_DAYS used by
/// the SessionEnd prune in runner::hook_lifecycle; both read the same override
/// vars so manual, session-end, and auto prune agree on the bound.
const RAW_AUTO_PRUNE_DEFAULT_RETENTION_DAYS: u64 = 14;

/// Minimum wall-clock gap between auto-prune sweeps on the capture hot path.
/// The store ages by whole days, so sweeping more often than a few hours buys
/// nothing; a stamp file under the store root throttles repeat runs.
const RAW_AUTO_PRUNE_INTERVAL_SECS: u64 = 6 * 60 * 60;

/// Minimum wall-clock gap between scans for interrupted staging directories.
/// Saving a run is a hot path; an old staging directory can safely wait for the
/// next bounded maintenance sweep instead of forcing every save to enumerate all
/// historical date directories.
const RAW_STAGING_CLEANUP_INTERVAL_SECS: u64 = 6 * 60 * 60;

/// Resolve the raw-output retention in days using the same precedence as the
/// SessionEnd prune: plugin userConfig env, then the operator env var, then the
/// default. `0` disables pruning. Kept local so the proxy hot path does not
/// depend on the runner module.
fn raw_auto_prune_retention_days() -> u64 {
    for var in [
        "CLAUDE_PLUGIN_OPTION_MEMORY_RETENTION_DAYS",
        "CLAUDE_SKILLS_RAW_RETENTION_DAYS",
    ] {
        if let Ok(value) = std::env::var(var) {
            if let Ok(parsed) = value.trim().parse::<u64>() {
                return parsed;
            }
        }
    }
    RAW_AUTO_PRUNE_DEFAULT_RETENTION_DAYS
}

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
    namespace: Option<RawNamespace>,
}

/// Optional owner identity used to scope reads to one workspace/session.
/// Existing unscoped callers remain compatible; governed callers should use
/// [`RawStore::with_namespace`] so a raw id cannot be replayed across tenants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawNamespace {
    pub workspace_id: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IntegrityManifest {
    schema_version: u32,
    raw_id: String,
    workspace_id: String,
    session_id: String,
    files: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct RawEntry {
    pub raw_id: String,
    pub path: PathBuf,
    pub meta: Option<RunMeta>,
}
#[cfg(unix)]
fn restrict_directory(path: &std::path::Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn restrict_directory(path: &std::path::Path) -> io::Result<()> {
    // Windows user-profile ACLs are inherited from the private profile root;
    // still verify the directory exists before publishing artifacts.
    fs::metadata(path).map(|_| ())
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    let mut permissions = file.metadata()?.permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    fs::write(path, bytes)
}

/// Persist UTF-8 metadata through the repository's atomic text writer, then
/// retain RawStore's private-file permissions on Unix. JSON manifests,
/// receipts, and compact metadata must not use a truncate-then-write sequence
/// that can leave readers with a partial file.
fn write_private_text_atomic(path: &std::path::Path, text: &str) -> io::Result<()> {
    crate::runtime::write_text(path, text).map_err(io::Error::other)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

impl RawStore {
    pub fn new() -> Self {
        let root = resolve_claude_home("")
            .map(|p| p.join("raw-output"))
            .unwrap_or_else(|_| std::env::temp_dir().join("keel-raw-output"));
        Self {
            root,
            namespace: None,
        }
    }

    pub fn with_root(root: PathBuf) -> Self {
        Self {
            root,
            namespace: None,
        }
    }

    pub fn with_namespace(root: PathBuf, namespace: RawNamespace) -> Self {
        Self {
            root,
            namespace: Some(namespace),
        }
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    pub fn save(&self, meta: &mut RunMeta, run: &RawRun) -> std::io::Result<()> {
        validate_raw_id(&meta.raw_id)?;
        self.validate_namespace()?;
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let day_dir = self.root.join(date);
        let dir = day_dir.join(&meta.raw_id);
        reject_path_components(&self.root)?;
        reject_path_components(&day_dir)?;
        fs::create_dir_all(&day_dir)?;
        restrict_directory(&self.root)?;
        restrict_directory(&day_dir)?;
        reject_symlink(&self.root)?;
        reject_symlink(&day_dir)?;
        cleanup_stale_raw_staging(&self.root);
        let staging_dir = day_dir.join(format!(
            ".tmp-{}-{}-{:08x}",
            meta.raw_id,
            std::process::id(),
            rand::random::<u32>()
        ));
        fs::create_dir(&staging_dir)?;
        restrict_directory(&staging_dir)?;

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
        let previous_raw_path = meta.raw_path.clone();
        meta.raw_path = dir.clone();
        let staged = (|| -> std::io::Result<()> {
            write_private(&staging_dir.join("stdout.log"), stdout_bytes)?;
            write_private(&staging_dir.join("stderr.log"), stderr_bytes)?;
            write_private(&staging_dir.join("command.txt"), meta.command.as_bytes())?;
            let meta_json = serde_json::to_string_pretty(meta)?;
            write_private_text_atomic(&staging_dir.join("meta.json"), &meta_json)?;
            write_integrity_manifest(
                &staging_dir,
                &meta.raw_id,
                &meta.workspace,
                self.namespace.as_ref(),
            )?;
            fs::rename(&staging_dir, &dir)
        })();
        if let Err(error) = staged {
            meta.raw_path = previous_raw_path;
            let _ = fs::remove_dir_all(&staging_dir);
            return Err(error);
        }
        Ok(())
    }

    pub fn save_compact(&self, meta: &RunMeta, compact_output: &str) -> std::io::Result<()> {
        validate_raw_id(&meta.raw_id)?;
        self.validate_namespace()?;
        if meta.raw_path.as_os_str().is_empty() {
            return Ok(());
        }
        if compact_output.len() > MAX_RAW_WRITE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "compact artifact exceeds maximum raw artifact size of {MAX_RAW_WRITE_BYTES} bytes"
                ),
            ));
        }
        let directory = self.find_dir(&meta.raw_id)?;
        validate_compact_path(&directory, &meta.compact_path)?;
        reject_path_components(&self.root)?;
        reject_path_components(&directory)?;
        restrict_directory(&self.root)?;
        restrict_directory(&directory)?;
        reject_symlink_if_exists(&meta.compact_path)?;
        write_private_text_atomic(&meta.compact_path, compact_output)?;
        let meta_json = serde_json::to_string_pretty(meta)?;
        reject_symlink_if_exists(&directory.join("meta.json"))?;
        write_private_text_atomic(&directory.join("meta.json"), &meta_json)?;
        self.refresh_integrity(&meta.raw_id)?;
        Ok(())
    }

    /// Persist the immutable execution receipt next to the raw artifact. The
    /// receipt is deliberately a separate schema-versioned file so existing
    /// `meta.json` consumers remain byte-compatible while every governed run
    /// gains an auditable identity and interception state.
    pub fn save_execution_receipt(
        &self,
        raw_id: &str,
        receipt: &ExecutionIdentity,
    ) -> std::io::Result<PathBuf> {
        validate_raw_id(raw_id)?;
        self.validate_namespace()?;
        let directory = self.find_dir(raw_id)?;
        restrict_directory(&self.root)?;
        restrict_directory(&directory)?;
        let path = directory.join("execution.json");
        reject_symlink_if_exists(&path)?;
        let serialized = serde_json::to_string_pretty(receipt)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        write_private_text_atomic(&path, &serialized)?;
        self.refresh_integrity(raw_id)?;
        Ok(path)
    }

    pub fn load_execution_receipt(&self, raw_id: &str) -> io::Result<ExecutionIdentity> {
        let directory = self.find_dir(raw_id)?;
        self.verify_integrity(&directory)?;
        let text = fs::read_to_string(directory.join("execution.json"))?;
        serde_json::from_str(&text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub fn save_screenshot(
        &self,
        raw_id: &str,
        command: &str,
        stdout: &[u8],
        stderr: &[u8],
        screenshot_png: &[u8],
        exit_code: i32,
    ) -> std::io::Result<PathBuf> {
        validate_raw_id(raw_id)?;
        self.validate_namespace()?;
        if screenshot_png.len() > MAX_RAW_WRITE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "screenshot exceeds maximum raw artifact size of {MAX_RAW_WRITE_BYTES} bytes"
                ),
            ));
        }
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let day_dir = self.root.join(date);
        let dir = day_dir.join(raw_id);
        reject_path_components(&self.root)?;
        reject_path_components(&day_dir)?;
        fs::create_dir_all(&day_dir)?;
        restrict_directory(&self.root)?;
        restrict_directory(&day_dir)?;
        reject_symlink(&self.root)?;
        reject_symlink(&day_dir)?;
        cleanup_stale_raw_staging(&self.root);
        let staging_dir = day_dir.join(format!(
            ".tmp-{}-{}-{:08x}",
            raw_id,
            std::process::id(),
            rand::random::<u32>()
        ));
        fs::create_dir(&staging_dir)?;
        restrict_directory(&staging_dir)?;

        // Bound direct callers too; rejected screenshots and text streams use
        // the same capture cap so no artifact grows without limit.
        let stdout_bytes = if stdout.len() > MAX_RAW_WRITE_BYTES {
            &stdout[..MAX_RAW_WRITE_BYTES]
        } else {
            stdout
        };
        let stderr_bytes = if stderr.len() > MAX_RAW_WRITE_BYTES {
            &stderr[..MAX_RAW_WRITE_BYTES]
        } else {
            stderr
        };

        let now = chrono::Local::now().timestamp_millis() as u64;
        let meta = RunMeta {
            raw_id: raw_id.to_string(),
            command: command.to_string(),
            program: "verify".to_string(),
            args: vec!["ui".to_string()],
            cwd: PathBuf::from("."),
            started_at: now,
            duration_ms: 0,
            exit_code,
            adapter_name: "verify-ui".to_string(),
            raw_path: dir.clone(),
            compact_path: PathBuf::new(),
            agent: "keel".to_string(),
            workspace: PathBuf::from("."),
            stdout_bytes: stdout.len(),
            stderr_bytes: stderr.len(),
            compact_stdout_bytes: 0,
            compact_stderr_bytes: 0,
            estimated_tokens_before: (stdout.len() + stderr.len()) / 4,
            estimated_tokens_after: 0,
            estimated_tokens_saved: 0,
            savings_pct: 0.0,
            compacted: false,
        };

        let staged = (|| -> std::io::Result<()> {
            write_private(&staging_dir.join("stdout.log"), stdout_bytes)?;
            write_private(&staging_dir.join("stderr.log"), stderr_bytes)?;
            write_private(&staging_dir.join("command.txt"), command.as_bytes())?;
            if !screenshot_png.is_empty() {
                write_private(&staging_dir.join("screenshot.png"), screenshot_png)?;
            }
            let meta_json = serde_json::to_string_pretty(&meta)?;
            write_private_text_atomic(&staging_dir.join("meta.json"), &meta_json)?;
            write_integrity_manifest(
                &staging_dir,
                raw_id,
                &meta.workspace,
                self.namespace.as_ref(),
            )?;
            fs::rename(&staging_dir, &dir)
        })();
        if let Err(error) = staged {
            let _ = fs::remove_dir_all(&staging_dir);
            return Err(error);
        }
        Ok(dir)
    }

    pub fn generate_id() -> String {
        let now = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let random: u32 = rand::random();
        format!("{now}-{:08x}", random)
    }

    pub fn find_dir(&self, raw_id: &str) -> io::Result<PathBuf> {
        validate_raw_id(raw_id)?;
        self.validate_namespace()?;
        let trimmed = raw_id.trim();
        // A caller-provided recovery root is part of the trust boundary. Do
        // not follow a symlinked root or date directory while resolving an id.
        reject_path_components(&self.root)?;
        reject_symlink(&self.root)?;
        for day in fs::read_dir(&self.root)? {
            let day = day?;
            let day_path = day.path();
            let day_metadata = fs::symlink_metadata(&day_path)?;
            if day_metadata.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "raw store date directory must not be a symlink",
                ));
            }
            if !day_metadata.file_type().is_dir() {
                continue;
            }
            let candidate = day_path.join(trimmed);
            if fs::symlink_metadata(&candidate)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "raw artifact directory must not be a symlink",
                ));
            }
            if fs::symlink_metadata(&candidate)
                .map(|metadata| metadata.file_type().is_dir())
                .unwrap_or(false)
            {
                self.enforce_namespace(&candidate)?;
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
        self.verify_integrity(&dir)?;
        let text = fs::read_to_string(dir.join("meta.json"))?;
        serde_json::from_str(&text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub fn read_file(&self, raw_id: &str, file_name: &str) -> io::Result<Vec<u8>> {
        let dir = self.find_dir(raw_id)?;
        validate_file_name(file_name)?;
        self.verify_integrity(&dir)?;
        let path = dir.join(file_name);
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_file() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "raw artifact file must not be a symlink or directory",
            ));
        }
        fs::read(path)
    }

    pub fn load_integrity(&self, raw_id: &str) -> io::Result<serde_json::Value> {
        let dir = self.find_dir(raw_id)?;
        self.verify_integrity(&dir)?;
        let text = fs::read_to_string(dir.join("integrity.json"))?;
        serde_json::from_str(&text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    fn enforce_namespace(&self, directory: &std::path::Path) -> io::Result<()> {
        let Some(manifest) = self.read_integrity_manifest(directory)? else {
            if self.namespace.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "raw artifact integrity manifest is required for namespaced reads",
                ));
            }
            return Ok(());
        };
        self.validate_manifest_owner(directory, &manifest)
    }

    fn validate_namespace(&self) -> io::Result<()> {
        let Some(namespace) = &self.namespace else {
            return Ok(());
        };
        if namespace.workspace_id.trim().is_empty()
            || namespace.session_id.trim().is_empty()
            || namespace.workspace_id.contains('\0')
            || namespace.session_id.contains('\0')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "raw store namespace requires non-empty workspace and session ids",
            ));
        }
        Ok(())
    }

    fn verify_integrity(&self, directory: &std::path::Path) -> io::Result<()> {
        let Some(manifest) = self.read_integrity_manifest(directory)? else {
            // Legacy unscoped artifacts predate manifests; governed namespaced
            // stores fail closed because `enforce_namespace` requires one.
            if self.namespace.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "raw artifact integrity manifest is required for namespaced reads",
                ));
            }
            return Ok(());
        };
        self.validate_manifest_owner(directory, &manifest)?;

        let mut actual_files = BTreeMap::new();
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let file_name = entry.file_name().to_string_lossy().to_string();
            if file_name == "integrity.json" {
                continue;
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if !metadata.file_type().is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("raw artifact entry is not regular: {file_name}"),
                ));
            }
            validate_file_name(&file_name)?;
            actual_files.insert(file_name, integrity_hash(&fs::read(path)?));
        }

        if actual_files.keys().ne(manifest.files.keys()) {
            let extras = actual_files
                .keys()
                .filter(|name| !manifest.files.contains_key(*name))
                .cloned()
                .collect::<Vec<_>>();
            let missing = manifest
                .files
                .keys()
                .filter(|name| !actual_files.contains_key(*name))
                .cloned()
                .collect::<Vec<_>>();
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "raw artifact integrity manifest file set mismatch (extra={extras:?}, missing={missing:?})"
                ),
            ));
        }

        for (file_name, expected) in manifest.files {
            validate_file_name(&file_name)?;
            let path = directory.join(&file_name);
            let metadata = fs::symlink_metadata(&path)?;
            if !metadata.file_type().is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("raw artifact file is not regular: {file_name}"),
                ));
            }
            let actual = integrity_hash(&fs::read(path)?);
            if actual != expected {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("raw artifact integrity mismatch: {file_name}"),
                ));
            }
        }
        Ok(())
    }

    fn read_integrity_manifest(
        &self,
        directory: &std::path::Path,
    ) -> io::Result<Option<IntegrityManifest>> {
        let path = directory.join("integrity.json");
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "raw integrity manifest must be a regular file",
            ));
        }
        let text = fs::read_to_string(path)?;
        let manifest: IntegrityManifest = serde_json::from_str(&text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if manifest.schema_version != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported raw integrity schema version",
            ));
        }
        Ok(Some(manifest))
    }

    fn validate_manifest_owner(
        &self,
        directory: &std::path::Path,
        manifest: &IntegrityManifest,
    ) -> io::Result<()> {
        let expected_raw_id = directory
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "raw artifact directory has no valid id",
                )
            })?;
        validate_raw_id(expected_raw_id).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("raw artifact directory id is invalid: {error}"),
            )
        })?;
        if manifest.raw_id != expected_raw_id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "raw integrity manifest id does not match artifact directory",
            ));
        }
        if let Some(namespace) = &self.namespace {
            if manifest.workspace_id != namespace.workspace_id
                || manifest.session_id != namespace.session_id
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "raw artifact namespace does not match requested workspace/session",
                ));
            }
        }
        Ok(())
    }

    fn refresh_integrity(&self, raw_id: &str) -> io::Result<()> {
        let directory = self.find_dir(raw_id)?;
        let meta = fs::read_to_string(directory.join("meta.json"))?;
        let workspace = serde_json::from_str::<RunMeta>(&meta)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
            .workspace;
        write_integrity_manifest(&directory, raw_id, &workspace, self.namespace.as_ref())
    }

    pub fn list(&self) -> io::Result<Vec<RawEntry>> {
        let mut entries = Vec::new();
        if !self.root.exists() {
            return Ok(entries);
        }
        self.validate_namespace()?;
        reject_path_components(&self.root)?;
        reject_symlink(&self.root)?;
        for day in fs::read_dir(&self.root)? {
            let day = day?;
            let day_path = day.path();
            let day_metadata = fs::symlink_metadata(&day_path)?;
            if day_metadata.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "raw store date directory must not be a symlink",
                ));
            }
            if !day_metadata.file_type().is_dir() {
                continue;
            }
            for raw in fs::read_dir(day_path)? {
                let raw = raw?;
                let raw_path = raw.path();
                let raw_metadata = fs::symlink_metadata(&raw_path)?;
                if raw_metadata.file_type().is_symlink() {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "raw artifact directory must not be a symlink",
                    ));
                }
                if !raw_metadata.file_type().is_dir() {
                    continue;
                }
                let raw_id = raw.file_name().to_string_lossy().to_string();
                if raw_id.starts_with(".tmp-") {
                    continue;
                }
                validate_raw_id(&raw_id).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid raw artifact id {raw_id:?}: {error}"),
                    )
                })?;
                if self.namespace.is_some() {
                    match self.verify_integrity(&raw_path) {
                        Ok(()) => {}
                        Err(error)
                            if matches!(
                                error.kind(),
                                io::ErrorKind::PermissionDenied | io::ErrorKind::NotFound
                            ) =>
                        {
                            continue
                        }
                        Err(error) => return Err(error),
                    }
                } else if self.read_integrity_manifest(&raw_path)?.is_some() {
                    self.verify_integrity(&raw_path)?;
                }
                let meta = fs::read_to_string(raw_path.join("meta.json"))
                    .ok()
                    .and_then(|text| serde_json::from_str::<RunMeta>(&text).ok());
                entries.push(RawEntry {
                    raw_id,
                    path: raw_path,
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
        let mut first_error = None;
        for entry in self.list()? {
            // why: date folders age by logical day even if mtime was touched.
            let age_signal = entry_age_signal(&entry.path).unwrap_or_else(|_| SystemTime::now());
            if age_signal < cutoff {
                match fs::remove_dir_all(&entry.path) {
                    Ok(()) => removed += 1,
                    Err(error) => {
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                }
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
        match first_error {
            Some(error) => Err(error),
            None => Ok(removed),
        }
    }

    /// Age-based prune for the capture hot path. Throttled to at most one sweep
    /// per RAW_AUTO_PRUNE_INTERVAL_SECS via a stamp file under the store root,
    /// and fail-open: a prune or stamp error never reaches the caller, so a
    /// housekeeping failure cannot fail or block the wrapped command. Uses the
    /// same `prune_older_than` the manual `raw prune` command uses, so manual,
    /// session-end, and auto prune never drift.
    pub fn auto_prune(&self) {
        let retention_days = raw_auto_prune_retention_days();
        if retention_days == 0 {
            return;
        }
        if !self.prune_stamp_due() {
            return;
        }
        if self.prune_older_than(retention_days).is_ok() {
            self.write_prune_stamp();
        }
    }

    /// True when no fresh stamp exists, meaning a sweep is due. A missing or
    /// unreadable stamp counts as due so a first run prunes.
    fn prune_stamp_due(&self) -> bool {
        let stamp = self.root.join(".last-auto-prune");
        let Ok(contents) = fs::read_to_string(&stamp) else {
            return true;
        };
        let Ok(secs) = contents.trim().parse::<u64>() else {
            return true;
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        now.saturating_sub(secs) >= RAW_AUTO_PRUNE_INTERVAL_SECS
    }

    fn write_prune_stamp(&self) {
        if reject_path_components(&self.root).is_err()
            || fs::create_dir_all(&self.root).is_err()
            || restrict_directory(&self.root).is_err()
            || reject_symlink_if_exists(&self.root.join(".last-auto-prune")).is_err()
        {
            return;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = write_private(
            &self.root.join(".last-auto-prune"),
            now.to_string().as_bytes(),
        );
    }
}

fn cleanup_stale_raw_staging(root: &std::path::Path) {
    let stamp = root.join(".last-staging-cleanup");
    if reject_path_components(root).is_err() || reject_symlink_if_exists(&stamp).is_err() {
        return;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    if let Ok(contents) = fs::read_to_string(&stamp) {
        if let Ok(previous) = contents.trim().parse::<u64>() {
            if now.saturating_sub(previous) < RAW_STAGING_CLEANUP_INTERVAL_SECS {
                return;
            }
        }
    }
    let Ok(day_directories) = fs::read_dir(root) else {
        return;
    };
    for day_directory in day_directories.flatten() {
        if !day_directory
            .file_type()
            .map(|kind| kind.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        let Ok(entries) = fs::read_dir(day_directory.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            if !file_name.starts_with(".tmp-") {
                continue;
            }
            let mut suffix_parts = file_name.rsplitn(3, '-');
            let nonce = suffix_parts.next().unwrap_or("");
            let process_id = suffix_parts.next().unwrap_or("");
            if nonce.is_empty() {
                continue;
            }
            let Ok(process_id) = process_id.parse::<u32>() else {
                continue;
            };
            if crate::runtime::process_is_alive(process_id) == Some(false) {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
    // The sweep is best-effort; a fresh stamp is written only after the walk,
    // so concurrent disappearance retries on the next save.
    let _ = write_private(&stamp, now.to_string().as_bytes());
}

fn reject_symlink(path: &std::path::Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("raw store path must not be a symlink: {}", path.display()),
        ));
    }
    Ok(())
}

/// Validate the target and, when it does not exist yet, the nearest existing
/// ancestor before a create/write operation. Calling `create_dir_all` first
/// would follow a symlinked raw-output root or date directory before the later
/// point check gets a chance to reject it. Do not walk beyond that ancestor:
/// macOS exposes `/var` as a symlink to `/private/var`, and benign system
/// aliases outside the configured raw-store boundary must remain usable.
fn reject_path_components(path: &std::path::Path) -> io::Result<()> {
    let mut current = path;
    loop {
        match fs::symlink_metadata(current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!(
                            "raw store path must not be a symlink: {}",
                            current.display()
                        ),
                    ));
                }
                return Ok(());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        current = current.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "raw store path has no existing ancestor: {}",
                    path.display()
                ),
            )
        })?;
    }
}

fn validate_file_name(file_name: &str) -> io::Result<()> {
    if file_name.is_empty()
        || file_name == "."
        || file_name == ".."
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid raw artifact file name",
        ));
    }
    Ok(())
}

fn validate_raw_id(raw_id: &str) -> io::Result<()> {
    let trimmed = raw_id.trim();
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed.contains("..")
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.chars().any(char::is_whitespace)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid raw id",
        ));
    }
    Ok(())
}

fn validate_compact_path(
    directory: &std::path::Path,
    compact_path: &std::path::Path,
) -> io::Result<()> {
    let file_name = compact_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "compact artifact path must name one file",
            )
        })?;
    validate_file_name(file_name)?;
    if matches!(file_name, "integrity.json" | "meta.json" | "execution.json") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "compact artifact path uses a reserved raw artifact file name",
        ));
    }
    let parent = compact_path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "compact artifact path must be inside the raw artifact directory",
        )
    })?;
    let expected = directory.canonicalize()?;
    let actual = parent.canonicalize()?;
    if actual != expected {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "compact artifact path escapes the raw artifact directory",
        ));
    }
    Ok(())
}

fn reject_symlink_if_exists(path: &std::path::Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "raw artifact target must not be a symlink: {}",
                        path.display()
                    ),
                ))
            } else if !metadata.file_type().is_file() {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "raw artifact target must be a regular file: {}",
                        path.display()
                    ),
                ))
            } else {
                Ok(())
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn integrity_hash(bytes: &[u8]) -> String {
    // Use the repository's stable FNV-1a helper without a new dependency; the
    // manifest detects tampering/corruption, not cryptographic authenticity.
    let mut hash: u64 = 14695981039346656037;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    format!("fnv1a64:{hash:016x}")
}

fn write_integrity_manifest(
    directory: &std::path::Path,
    raw_id: &str,
    workspace: &std::path::Path,
    namespace: Option<&RawNamespace>,
) -> io::Result<()> {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "integrity.json" || name.starts_with('.') {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        validate_file_name(&name)?;
        if !metadata.file_type().is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("raw artifact entry must be a regular file: {name}"),
            ));
        }
        files.insert(name, integrity_hash(&fs::read(entry.path())?));
    }
    let manifest = IntegrityManifest {
        schema_version: 1,
        raw_id: raw_id.to_string(),
        workspace_id: namespace
            .map(|value| value.workspace_id.clone())
            .unwrap_or_else(|| workspace.to_string_lossy().to_string()),
        session_id: namespace
            .map(|value| value.session_id.clone())
            .unwrap_or_default(),
        files,
    };
    let serialized = serde_json::to_string_pretty(&manifest)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    write_private_text_atomic(&directory.join("integrity.json"), &serialized)
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
    use super::{parse_yyyy_mm_dd_midnight_utc, RawNamespace, RawRun, RawStore, RunMeta};
    use std::path::PathBuf;

    fn sample_meta(raw_id: &str) -> RunMeta {
        RunMeta {
            raw_id: raw_id.to_string(),
            command: "test".to_string(),
            program: "test".to_string(),
            args: Vec::new(),
            cwd: PathBuf::from("."),
            started_at: 1,
            duration_ms: 2,
            exit_code: 0,
            adapter_name: "tests".to_string(),
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
        }
    }

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
        let initially_loaded = store.load_meta(&meta.raw_id).expect("load initial meta");
        assert_eq!(initially_loaded.raw_path, meta.raw_path);
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
    fn raw_store_save_removes_only_dead_process_staging_directories() {
        let root =
            std::env::temp_dir().join(format!("keel-raw-staging-cleanup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let day = chrono::Local::now().format("%Y-%m-%d").to_string();
        let missing_pid = i32::MAX as u32;
        let stale = root.join(&day).join(format!(".tmp-old-{missing_pid}-a1"));
        let prior_stale = root
            .join("2001-02-03")
            .join(format!(".tmp-old-{missing_pid}-c3"));
        let live = root
            .join(&day)
            .join(format!(".tmp-live-{}-b2", std::process::id()));
        std::fs::create_dir_all(&stale).expect("stale staging");
        std::fs::create_dir_all(&prior_stale).expect("prior-day stale staging");
        std::fs::create_dir_all(&live).expect("live staging");
        let store = RawStore::with_root(root.clone());
        let mut meta = sample_meta("staging-cleanup");
        let run = RawRun {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code: 0,
        };

        store.save(&mut meta, &run).expect("save");

        assert!(!stale.exists());
        assert!(
            !prior_stale.exists(),
            "interrupted staging from an earlier day must be reclaimed"
        );
        assert!(live.exists(), "active process staging must be preserved");
        assert!(
            store
                .list()
                .expect("list")
                .iter()
                .all(|entry| !entry.raw_id.starts_with(".tmp-")),
            "in-progress staging directories must not be exposed as raw runs"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn raw_store_staging_cleanup_is_throttled_by_stamp() {
        let root =
            std::env::temp_dir().join(format!("keel-raw-staging-throttle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let day = chrono::Local::now().format("%Y-%m-%d").to_string();
        let missing_pid = i32::MAX as u32;
        let stale = root
            .join(&day)
            .join(format!(".tmp-old-{missing_pid}-throttle"));
        std::fs::create_dir_all(&stale).expect("stale staging");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        std::fs::write(root.join(".last-staging-cleanup"), now.to_string())
            .expect("fresh cleanup stamp");

        let store = RawStore::with_root(root.clone());
        let mut meta = sample_meta("staging-throttle");
        store
            .save(
                &mut meta,
                &RawRun {
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    exit_code: 0,
                },
            )
            .expect("save with fresh cleanup stamp");
        assert!(
            stale.exists(),
            "a fresh staging stamp must avoid another historical scan"
        );

        let old = now.saturating_sub(super::RAW_STAGING_CLEANUP_INTERVAL_SECS + 60);
        std::fs::write(root.join(".last-staging-cleanup"), old.to_string())
            .expect("expired cleanup stamp");
        let mut second_meta = sample_meta("staging-throttle-second");
        store
            .save(
                &mut second_meta,
                &RawRun {
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    exit_code: 0,
                },
            )
            .expect("save after cleanup interval");
        assert!(!stale.exists(), "an expired stamp must permit cleanup");
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn raw_store_artifacts_are_private_at_rest() {
        use std::os::unix::fs::PermissionsExt;

        fn mode(path: &std::path::Path) -> u32 {
            std::fs::metadata(path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777
        }

        let root =
            std::env::temp_dir().join(format!("keel-raw-store-permissions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = RawStore::with_root(root.clone());
        let mut meta = RunMeta {
            raw_id: "20260512-permissions".to_string(),
            command: "printf secret".to_string(),
            program: "printf".to_string(),
            args: vec!["secret".to_string()],
            cwd: PathBuf::from("."),
            started_at: 1,
            duration_ms: 2,
            exit_code: 0,
            adapter_name: "generic".to_string(),
            raw_path: PathBuf::new(),
            compact_path: PathBuf::new(),
            agent: "test".to_string(),
            workspace: PathBuf::from("."),
            stdout_bytes: 6,
            stderr_bytes: 0,
            compact_stdout_bytes: 0,
            compact_stderr_bytes: 0,
            estimated_tokens_before: 1,
            estimated_tokens_after: 1,
            estimated_tokens_saved: 0,
            savings_pct: 0.0,
            compacted: false,
        };
        store
            .save(
                &mut meta,
                &RawRun {
                    stdout: b"secret".to_vec(),
                    stderr: Vec::new(),
                    exit_code: 0,
                },
            )
            .expect("save");
        meta.compact_path = meta.raw_path.join("compact.txt");
        store.save_compact(&meta, "secret").expect("compact");
        store.write_prune_stamp();

        let day = meta.raw_path.parent().expect("day");
        assert_eq!(mode(&root), 0o700);
        assert_eq!(mode(day), 0o700);
        assert_eq!(mode(&meta.raw_path), 0o700);
        for file in [
            "stdout.log",
            "stderr.log",
            "command.txt",
            "meta.json",
            "compact.txt",
        ] {
            assert_eq!(mode(&meta.raw_path.join(file)), 0o600, "{file}");
        }
        assert_eq!(mode(&root.join(".last-auto-prune")), 0o600);
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
    fn raw_store_save_rejects_traversal_before_creating_directories() {
        let root = crate::test_support::unique_temp_dir("keel-raw-save-traversal");
        let store = RawStore::with_root(root.to_path_buf());
        let mut meta = sample_meta("../escaped");
        let error = store
            .save(
                &mut meta,
                &RawRun {
                    stdout: b"must not write".to_vec(),
                    stderr: Vec::new(),
                    exit_code: 0,
                },
            )
            .expect_err("save must reject an untrusted raw id");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!root.join("2026-05-12").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn raw_store_compact_path_must_stay_inside_the_raw_artifact() {
        let root = crate::test_support::unique_temp_dir("keel-raw-compact-scope");
        let store = RawStore::with_root(root.to_path_buf());
        let raw_id = "20260512-143012-compact0001";
        let mut meta = sample_meta(raw_id);
        store
            .save(
                &mut meta,
                &RawRun {
                    stdout: b"raw".to_vec(),
                    stderr: Vec::new(),
                    exit_code: 0,
                },
            )
            .expect("save");
        meta.compact_path = root.join("outside.txt");
        let error = store
            .save_compact(&meta, "must not write")
            .expect_err("compact path must be scoped");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(!root.join("outside.txt").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn screenshot_size_is_bounded_before_publishing_artifact() {
        let root = crate::test_support::unique_temp_dir("keel-raw-screenshot-cap");
        let store = RawStore::with_root(root.to_path_buf());
        let raw_id = "20260512-143012-screenshot0001";
        let error = store
            .save_screenshot(
                raw_id,
                "keel verify ui",
                &[],
                &[],
                &vec![b'x'; super::MAX_RAW_WRITE_BYTES + 1],
                0,
            )
            .expect_err("oversized screenshot must fail closed");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!root.join(raw_id).exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn screenshot_text_streams_are_capped_at_the_raw_store_boundary() {
        let root = crate::test_support::unique_temp_dir("keel-raw-screenshot-stream-cap");
        let store = RawStore::with_root(root.to_path_buf());
        let raw_id = "20260512-143012-screenshot0002";
        let stdout = vec![b'x'; super::MAX_RAW_WRITE_BYTES + 1];
        let dir = store
            .save_screenshot(raw_id, "keel verify ui", &stdout, &[], &[], 0)
            .expect("oversized text stream should be capped");
        assert_eq!(
            std::fs::metadata(dir.join("stdout.log"))
                .expect("stdout metadata")
                .len() as usize,
            super::MAX_RAW_WRITE_BYTES
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn compact_save_validates_raw_id_before_empty_path_noop() {
        let root = crate::test_support::unique_temp_dir("keel-raw-compact-id");
        let store = RawStore::with_root(root.to_path_buf());
        let meta = sample_meta("../invalid");
        let error = store
            .save_compact(&meta, "compact")
            .expect_err("invalid raw ids must be rejected even when no path is set");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        let _ = std::fs::remove_dir_all(root);
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

    /// Serialize the auto-prune tests that mutate the shared retention env vars
    /// so parallel test threads do not observe each other's override.
    static AUTO_PRUNE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    const RETENTION_ENV_VARS: [&str; 2] = [
        "CLAUDE_PLUGIN_OPTION_MEMORY_RETENTION_DAYS",
        "CLAUDE_SKILLS_RAW_RETENTION_DAYS",
    ];

    /// Set the operator retention env var, run the closure, restore prior state.
    /// Caller must hold AUTO_PRUNE_ENV_LOCK.
    fn with_retention_env<F: FnOnce() -> R, R>(value: Option<&str>, run: F) -> R {
        let previous: Vec<Option<String>> = RETENTION_ENV_VARS
            .iter()
            .map(|var| std::env::var(var).ok())
            .collect();
        std::env::remove_var(RETENTION_ENV_VARS[0]);
        match value {
            Some(v) => std::env::set_var(RETENTION_ENV_VARS[1], v),
            None => std::env::remove_var(RETENTION_ENV_VARS[1]),
        }
        let result = run();
        for (index, var) in RETENTION_ENV_VARS.iter().enumerate() {
            match &previous[index] {
                Some(v) => std::env::set_var(var, v),
                None => std::env::remove_var(var),
            }
        }
        result
    }

    fn auto_prune_root(tag: &str) -> crate::test_support::TestTempDir {
        crate::test_support::unique_temp_dir(&format!("keel-raw-auto-prune-{tag}"))
    }

    /// Write one raw entry under a synthetic logical day so the prune ages it by
    /// the date folder, independent of filesystem mtime.
    fn write_dated_entry(root: &std::path::Path, day: &str, id: &str) -> PathBuf {
        let dir = root.join(day).join(id);
        std::fs::create_dir_all(&dir).expect("entry dir");
        std::fs::write(dir.join("stdout.log"), b"x").expect("stdout");
        dir
    }

    #[test]
    fn auto_prune_removes_stale_keeps_fresh() {
        let _guard = AUTO_PRUNE_ENV_LOCK.lock().unwrap();
        with_retention_env(Some("14"), || {
            let root = auto_prune_root("stale");
            let store = RawStore::with_root(root.to_path_buf());
            let stale = write_dated_entry(&root, "2001-02-03", "stale-id");
            let fresh = write_dated_entry(&root, "2099-01-01", "fresh-id");
            store.auto_prune();
            assert!(!stale.exists(), "stale entry removed by auto prune");
            assert!(fresh.exists(), "fresh entry kept by auto prune");
            let _ = std::fs::remove_dir_all(&root);
        });
    }

    #[test]
    fn auto_prune_disabled_when_retention_is_zero() {
        let _guard = AUTO_PRUNE_ENV_LOCK.lock().unwrap();
        with_retention_env(Some("0"), || {
            let root = auto_prune_root("disabled");
            let store = RawStore::with_root(root.to_path_buf());
            let ancient = write_dated_entry(&root, "2001-02-03", "ancient-id");
            store.auto_prune();
            assert!(
                ancient.exists(),
                "retention=0 must disable the auto prune even for an ancient entry"
            );
            let _ = std::fs::remove_dir_all(&root);
        });
    }

    #[test]
    fn auto_prune_throttles_to_once_per_interval() {
        let _guard = AUTO_PRUNE_ENV_LOCK.lock().unwrap();
        with_retention_env(Some("14"), || {
            let root = auto_prune_root("throttle");
            let store = RawStore::with_root(root.to_path_buf());
            let stamp = root.join(".last-auto-prune");
            std::fs::create_dir_all(&root).expect("root");
            // A fresh stamp (now) means a sweep is not due, so even a stale
            // entry survives this call.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            std::fs::write(&stamp, now.to_string()).expect("write fresh stamp");
            let stale = write_dated_entry(&root, "2001-02-03", "stale-id");
            store.auto_prune();
            assert!(
                stale.exists(),
                "a fresh stamp must throttle the sweep within the interval"
            );

            // An old stamp (older than the interval) means a sweep is due.
            let old = now.saturating_sub(super::RAW_AUTO_PRUNE_INTERVAL_SECS + 60);
            std::fs::write(&stamp, old.to_string()).expect("write old stamp");
            store.auto_prune();
            assert!(
                !stale.exists(),
                "an expired stamp must let the sweep run again"
            );
            let _ = std::fs::remove_dir_all(&root);
        });
    }

    #[test]
    fn auto_prune_fails_open_when_store_is_unwritable() {
        let _guard = AUTO_PRUNE_ENV_LOCK.lock().unwrap();
        with_retention_env(Some("14"), || {
            // A regular file in the would-be directory path makes both listing
            // and stamp creation fail without relying on platform permissions.
            let root = auto_prune_root("unwritable");
            let blocker = root.join("not-a-directory");
            std::fs::write(&blocker, "block").expect("create path blocker");
            let store_root = blocker.join("raw");
            let store = RawStore::with_root(store_root.clone());
            store.auto_prune();
            assert!(!store_root.exists());
        });
    }

    #[test]
    fn integrity_manifest_detects_tampering_and_namespace_mismatch() {
        let root = crate::test_support::unique_temp_dir("keel-raw-integrity");
        let namespace = RawNamespace {
            workspace_id: "workspace-a".to_string(),
            session_id: "session-a".to_string(),
        };
        let store = RawStore::with_namespace(root.to_path_buf(), namespace.clone());
        let raw_id = "20260512-143012-a1b2c3d4";
        let mut meta = sample_meta(raw_id);
        meta.workspace = PathBuf::from("workspace-a");
        store
            .save(
                &mut meta,
                &RawRun {
                    stdout: b"stable".to_vec(),
                    stderr: Vec::new(),
                    exit_code: 0,
                },
            )
            .expect("save");
        let manifest = store.load_integrity(raw_id).expect("manifest");
        assert_eq!(manifest["schema_version"], 1);
        assert_eq!(
            store
                .read_file(raw_id, "stdout.log")
                .expect("authorized read"),
            b"stable"
        );

        std::fs::write(meta.raw_path.join("stdout.log"), b"tampered").expect("tamper");
        let error = store
            .read_file(raw_id, "stdout.log")
            .expect_err("tamper must fail closed");
        assert!(error.to_string().contains("integrity mismatch"));

        let other = RawStore::with_namespace(
            root.to_path_buf(),
            RawNamespace {
                workspace_id: "workspace-b".to_string(),
                session_id: "session-b".to_string(),
            },
        );
        let error = other.find_dir(raw_id).expect_err("cross namespace read");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn integrity_manifest_rejects_extra_files_and_directory_id_drift() {
        let root = crate::test_support::unique_temp_dir("keel-raw-integrity-shape");
        let store = RawStore::with_root(root.to_path_buf());
        let raw_id = "20260512-143012-shape0001";
        let mut meta = sample_meta(raw_id);
        store
            .save(
                &mut meta,
                &RawRun {
                    stdout: b"stable".to_vec(),
                    stderr: Vec::new(),
                    exit_code: 0,
                },
            )
            .expect("save");

        std::fs::write(meta.raw_path.join("unexpected.log"), b"unlisted").expect("extra file");
        let error = store
            .load_meta(raw_id)
            .expect_err("unlisted files must fail closed");
        assert!(error.to_string().contains("file set mismatch"));
        std::fs::remove_file(meta.raw_path.join("unexpected.log")).expect("remove extra");

        let integrity_path = meta.raw_path.join("integrity.json");
        let mut manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&integrity_path).expect("manifest text"))
                .expect("manifest json");
        manifest["raw_id"] = serde_json::Value::String("different-id".to_string());
        std::fs::write(
            &integrity_path,
            serde_json::to_string_pretty(&manifest).expect("serialize manifest"),
        )
        .expect("rewrite manifest");
        let error = store
            .load_meta(raw_id)
            .expect_err("manifest id drift must fail closed");
        assert!(error.to_string().contains("manifest id"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn namespaced_list_does_not_expose_other_session_entries() {
        let root = crate::test_support::unique_temp_dir("keel-raw-list-namespace");
        let raw_id = "20260512-143012-list000001";
        let mut meta = sample_meta(raw_id);
        meta.workspace = PathBuf::from("workspace-a");
        RawStore::with_namespace(
            root.to_path_buf(),
            RawNamespace {
                workspace_id: "workspace-a".to_string(),
                session_id: "session-a".to_string(),
            },
        )
        .save(
            &mut meta,
            &RawRun {
                stdout: b"owned".to_vec(),
                stderr: Vec::new(),
                exit_code: 0,
            },
        )
        .expect("save owned entry");

        let other = RawStore::with_namespace(
            root.to_path_buf(),
            RawNamespace {
                workspace_id: "workspace-b".to_string(),
                session_id: "session-b".to_string(),
            },
        );
        assert!(other.list().expect("list other namespace").is_empty());
        let owned = RawStore::with_namespace(
            root.to_path_buf(),
            RawNamespace {
                workspace_id: "workspace-a".to_string(),
                session_id: "session-a".to_string(),
            },
        )
        .list()
        .expect("list owner namespace");
        assert_eq!(
            owned
                .iter()
                .map(|entry| entry.raw_id.as_str())
                .collect::<Vec<_>>(),
            [raw_id]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unscoped_legacy_artifacts_remain_readable_without_a_manifest() {
        let root = crate::test_support::unique_temp_dir("keel-raw-legacy");
        let raw_id = "20260512-143012-legacy0001";
        let directory = root.join("2026-05-12").join(raw_id);
        std::fs::create_dir_all(&directory).expect("legacy directory");
        let mut meta = sample_meta(raw_id);
        meta.raw_path = directory.clone();
        std::fs::write(
            directory.join("meta.json"),
            serde_json::to_string_pretty(&meta).expect("meta json"),
        )
        .expect("meta");
        std::fs::write(directory.join("stdout.log"), b"legacy output").expect("stdout");

        let unscoped = RawStore::with_root(root.to_path_buf());
        assert_eq!(
            unscoped.load_meta(raw_id).expect("legacy metadata").raw_id,
            raw_id
        );
        assert_eq!(
            unscoped
                .read_file(raw_id, "stdout.log")
                .expect("legacy output"),
            b"legacy output"
        );

        let namespaced = RawStore::with_namespace(
            root.to_path_buf(),
            RawNamespace {
                workspace_id: "workspace".to_string(),
                session_id: "session".to_string(),
            },
        );
        let error = namespaced
            .load_meta(raw_id)
            .expect_err("governed reads must require a manifest");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn raw_artifact_file_names_are_scoped() {
        let root = crate::test_support::unique_temp_dir("keel-raw-file-scope");
        let store = RawStore::with_root(root.to_path_buf());
        let raw_id = "20260512-143012-a1b2c3d4";
        let mut meta = sample_meta(raw_id);
        store
            .save(
                &mut meta,
                &RawRun {
                    stdout: b"ok".to_vec(),
                    stderr: Vec::new(),
                    exit_code: 0,
                },
            )
            .expect("save");
        for file_name in ["../meta.json", "nested/file", "..\\meta.json"] {
            let error = store
                .read_file(raw_id, file_name)
                .expect_err("path traversal must fail");
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn raw_store_rejects_symlinked_root_and_date_before_writing() {
        use std::os::unix::fs::symlink;

        let base = crate::test_support::unique_temp_dir("keel-raw-symlink-write");
        let target = base.join("target");
        std::fs::create_dir_all(&target).expect("target");

        let root_link = base.join("root-link");
        symlink(&target, &root_link).expect("root symlink");
        let store = RawStore::with_root(root_link.clone());
        let mut meta = sample_meta("symlink-root");
        let error = store
            .save(
                &mut meta,
                &RawRun {
                    stdout: b"must not write".to_vec(),
                    stderr: Vec::new(),
                    exit_code: 0,
                },
            )
            .expect_err("symlinked root must fail closed");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(
            std::fs::read_dir(&target).expect("target entries").count(),
            0
        );

        let root = base.join("root");
        std::fs::create_dir_all(&root).expect("root");
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let date_target = base.join("date-target");
        std::fs::create_dir_all(&date_target).expect("date target");
        symlink(&date_target, root.join(&date)).expect("date symlink");
        let store = RawStore::with_root(root);
        let mut meta = sample_meta("symlink-date");
        let error = store
            .save(
                &mut meta,
                &RawRun {
                    stdout: b"must not write".to_vec(),
                    stderr: Vec::new(),
                    exit_code: 0,
                },
            )
            .expect_err("symlinked date directory must fail closed");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(
            std::fs::read_dir(&date_target)
                .expect("date target entries")
                .count(),
            0
        );
    }

    #[cfg(windows)]
    #[test]
    fn prune_continues_after_locked_entry_and_auto_prune_retries() {
        use std::os::windows::fs::OpenOptionsExt;

        let _guard = AUTO_PRUNE_ENV_LOCK.lock().unwrap();
        with_retention_env(Some("14"), || {
            let root = auto_prune_root("locked-entry");
            let store = RawStore::with_root(root.to_path_buf());
            let blocked = write_dated_entry(&root, "2001-02-03", "zzz-blocked");
            let removable = write_dated_entry(&root, "2001-02-03", "aaa-removable");
            let lock = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(blocked.join("stdout.log"))
                .expect("exclusive lock");

            store.auto_prune();
            assert!(
                blocked.exists(),
                "locked entry must survive the failed removal"
            );
            assert!(
                !removable.exists(),
                "a locked sibling must not abort the rest of the prune sweep"
            );
            assert!(
                !root.join(".last-auto-prune").exists(),
                "a partial sweep must not be stamped successful"
            );

            drop(lock);
            store.auto_prune();
            assert!(
                !blocked.exists(),
                "without a success stamp, the next call must retry the locked entry"
            );
            assert!(root.join(".last-auto-prune").exists());
            let _ = std::fs::remove_dir_all(&root);
        });
    }
}
