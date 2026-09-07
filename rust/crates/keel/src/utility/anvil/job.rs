use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;

use crate::runtime::resolve_claude_home;
use crate::utility::anvil::lock::validate_lock;
use crate::utility::system_map::workspace_key;

pub struct JobLease {
    path: PathBuf,
    owner: String,
}

impl JobLease {
    pub fn acquire(paths: &JobPaths) -> Result<Self, String> {
        let parent = paths
            .dir
            .parent()
            .ok_or_else(|| "anvil: job directory has no parent".to_string())?;
        std::fs::create_dir_all(parent).map_err(|error| format!("anvil lock: {error}"))?;
        let path = parent.join("anvil.operation.lock");
        let owner = format!(
            "pid={} nonce={}\n",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        );
        for attempt in 0..2 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    use std::io::Write as _;
                    if let Err(error) = file
                        .write_all(owner.as_bytes())
                        .and_then(|_| file.sync_all())
                    {
                        drop(file);
                        let _ = std::fs::remove_file(&path); // intentional partial-lease cleanup
                        return Err(format!("anvil lock: {error}"));
                    }
                    return Ok(Self { path, owner });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && attempt == 0 => {
                    let stale = std::fs::metadata(&path)
                        .and_then(|metadata| metadata.modified())
                        .ok()
                        .and_then(|modified| modified.elapsed().ok())
                        .is_some_and(|age| age > std::time::Duration::from_secs(7_200));
                    if stale {
                        std::fs::remove_file(&path).map_err(|remove| {
                            format!("anvil lock: remove stale lease: {remove}")
                        })?;
                        continue;
                    }
                    return Err(format!("anvil: another operation owns {}", path.display()));
                }
                Err(error) => return Err(format!("anvil lock: {error}")),
            }
        }
        Err("anvil: could not acquire operation lease".to_string())
    }
}

impl Drop for JobLease {
    fn drop(&mut self) {
        if std::fs::read_to_string(&self.path).ok().as_deref() == Some(self.owner.as_str()) {
            let _ = std::fs::remove_file(&self.path); // intentional best-effort Drop cleanup
        }
    }
}

/// Host-neutral Anvil bank for one workspace.
///
/// Lock, prefix, gates, cast results, and the report live under
/// `<keel-home>/memories/workspaces/<slug>/anvil/` — the same per-workspace
/// memory lane SYSTEM_MAP and code-graph already use. The user workspace is
/// never written. Any CLI that shares KEEL_HOME / `--claude-home` resumes the
/// same progress.
#[derive(Debug, Clone)]
pub struct JobPaths {
    pub workspace: PathBuf,
    pub home: PathBuf,
    pub dir: PathBuf,
}

impl JobPaths {
    pub fn resolve(workspace_flag: &str, claude_home_flag: &str) -> Result<Self, String> {
        let workspace = resolve_workspace_root(workspace_flag)?;
        let home = resolve_claude_home(claude_home_flag)?;
        Ok(Self::from_resolved(workspace, home))
    }

    pub fn from_resolved(workspace: PathBuf, home: PathBuf) -> Self {
        let slug = workspace_key(&workspace.to_string_lossy());
        let preferred = home
            .join("memories")
            .join("workspaces")
            .join(slug)
            .join("anvil");
        let dir = migrate_anvil_lane(&home, &workspace, &preferred);
        Self {
            workspace,
            home,
            dir,
        }
    }

    pub fn ensure_dir(&self) -> Result<&Path, String> {
        std::fs::create_dir_all(&self.dir).map_err(|error| format!("anvil dir: {error}"))?;
        Ok(&self.dir)
    }

    pub fn lock_path(&self) -> PathBuf {
        self.dir.join("anvil.lock.json")
    }

    pub fn prefix_path(&self) -> PathBuf {
        self.dir.join("prefix.md")
    }

    pub fn prefix_hash_path(&self) -> PathBuf {
        self.dir.join("prefix.sha256")
    }

    pub fn report_path(&self) -> PathBuf {
        self.dir.join("anvil.report.json")
    }

    pub fn out_dir(&self) -> PathBuf {
        self.dir.join("anvil_out")
    }

    pub fn gates_dir(&self) -> PathBuf {
        self.dir.join("gates")
    }

    pub fn clarify_packet_path(&self) -> PathBuf {
        crate::utility::anvil::clarify::clarify_packet_path(&self.dir)
    }

    pub fn clarify_required_path(&self) -> PathBuf {
        crate::utility::anvil::clarify::clarify_required_path(&self.dir)
    }
}

pub fn resolve_workspace_root(flag: &str) -> Result<PathBuf, String> {
    let path = crate::runtime::resolve_repository_root(flag)
        .map_err(|error| format!("workspace-root: {error}"))?;
    if !path.is_dir() {
        return Err(format!(
            "workspace-root not a directory: {}",
            path.display()
        ));
    }
    Ok(path)
}

pub fn env_model(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

pub fn load_lock(paths: &JobPaths) -> Result<JsonValue, String> {
    let path = paths.lock_path();
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("anvil: missing lock at {}: {error}", path.display()))?;
    validate_lock(&text)
}

/// Truthful anvil axis for the stats/observe dashboards: one `(id, state)`
/// pair per workspace lane holding an anvil lock under
/// `<home>/memories/workspaces/<slug>/anvil/`. `workspace_root` scopes the
/// lookup to that single workspace; `None` scans every lane. A job is
/// "complete" only when its `anvil.report.json` exists, "active" otherwise.
/// Returns an empty vec when no job was run; callers omit the axis instead of
/// printing a fabricated none-active placeholder.
pub fn active_jobs_summary(
    claude_home: &Path,
    workspace_root: Option<&Path>,
) -> Vec<(String, String)> {
    let workspaces = claude_home.join("memories").join("workspaces");
    let lanes: Vec<PathBuf> = match workspace_root {
        Some(root) => {
            vec![JobPaths::from_resolved(root.to_path_buf(), claude_home.to_path_buf()).dir]
        }
        None => std::fs::read_dir(&workspaces)
            .map(|entries| {
                entries
                    .filter_map(std::result::Result::ok)
                    .map(|entry| entry.path().join("anvil"))
                    .filter(|lane| lane.is_dir())
                    .collect()
            })
            .unwrap_or_default(),
    };
    let mut jobs = Vec::new();
    for lane in lanes {
        if !lane.join("anvil.lock.json").is_file() {
            continue;
        }
        let id = lane
            .parent()
            .and_then(|parent| parent.file_name())
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        let state = if lane.join("anvil.report.json").is_file() {
            "complete"
        } else {
            "active"
        };
        jobs.push((id, state.to_string()));
    }
    jobs.sort();
    jobs
}

fn migrate_anvil_lane(home: &Path, workspace: &Path, preferred: &Path) -> PathBuf {
    if preferred.is_dir() {
        return preferred.to_path_buf();
    }
    let workspace_text = workspace.to_string_lossy();
    for alias in crate::utility::system_map::workspace_key_aliases(&workspace_text)
        .into_iter()
        .skip(1)
    {
        let legacy = home
            .join("memories")
            .join("workspaces")
            .join(alias)
            .join("anvil");
        if !legacy.is_dir() {
            continue;
        }
        if copy_directory_missing(&legacy, preferred).is_ok() {
            return preferred.to_path_buf();
        }
        return legacy;
    }
    preferred.to_path_buf()
}

fn copy_directory_missing(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            copy_directory_missing(&entry.path(), &target)?;
        } else if file_type.is_file() && !target.exists() {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct PieceSpec {
    pub id: String,
    pub files: Vec<String>,
    pub gates: Vec<String>,
    pub critic: String,
}

pub fn pieces_from_lock(lock: &JsonValue, only: &str) -> Result<Vec<PieceSpec>, String> {
    let rows = lock
        .get("pieces")
        .and_then(JsonValue::as_array)
        .ok_or("anvil: lock has no pieces")?;
    let mut out = Vec::new();
    for row in rows {
        let id = row
            .get("id")
            .and_then(JsonValue::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if id.is_empty() {
            return Err("anvil: piece id missing".into());
        }
        if !only.is_empty() && only != id {
            continue;
        }
        let files = string_list(row.get("files"));
        let gates = string_list(row.get("gates"));
        let critic = row
            .get("critic")
            .and_then(JsonValue::as_str)
            .unwrap_or("none")
            .to_string();
        out.push(PieceSpec {
            id,
            files,
            gates,
            critic,
        });
    }
    if out.is_empty() {
        return Err(if only.is_empty() {
            "anvil: lock pieces empty".into()
        } else {
            format!("anvil: piece {only:?} not in lock")
        });
    }
    Ok(out)
}

pub fn n_casts(lock: &JsonValue) -> u64 {
    lock.get("budget")
        .and_then(|budget| budget.get("n_casts"))
        .and_then(JsonValue::as_u64)
        .unwrap_or(3)
        .clamp(1, 8)
}

pub fn generation(lock: &JsonValue) -> Result<&str, String> {
    lock.get("generation")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "anvil: lock generation missing".to_string())
}

#[derive(Debug, Clone, Copy)]
pub struct JobBudget {
    pub builder_retries: u64,
    pub max_tokens_cast: u64,
    pub max_tokens_loop: u64,
    pub max_tool_chars: usize,
    pub max_iterations: usize,
    pub min_improvement: f64,
    pub wall_timeout: std::time::Duration,
    pub gate_timeout: std::time::Duration,
}

pub fn budget_from_lock(lock: &JsonValue) -> Result<JobBudget, String> {
    let budget = lock
        .get("budget")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| "anvil: lock budget missing".to_string())?;
    let required = |key: &str| {
        budget
            .get(key)
            .and_then(JsonValue::as_u64)
            .ok_or_else(|| format!("anvil: lock budget.{key} missing"))
    };
    Ok(JobBudget {
        builder_retries: required("builder_retries")?,
        max_tokens_cast: required("max_tokens_cast")?,
        max_tokens_loop: required("max_tokens_loop")?,
        max_tool_chars: required("max_tool_chars")? as usize,
        max_iterations: required("max_iterations")? as usize,
        min_improvement: budget
            .get("min_improvement_threshold")
            .and_then(JsonValue::as_f64)
            .ok_or_else(|| "anvil: lock budget.min_improvement_threshold missing".to_string())?,
        wall_timeout: std::time::Duration::from_secs(required("wall_timeout_secs")?),
        gate_timeout: std::time::Duration::from_secs(required("gate_timeout_secs")?),
    })
}

fn string_list(value: Option<&JsonValue>) -> Vec<String> {
    value
        .and_then(JsonValue::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(JsonValue::as_str)
                .map(|item| item.to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempTestDir(std::path::PathBuf);

    impl TempTestDir {
        fn new(label: &str) -> Self {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("anvil-{label}-{}-{stamp}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempTestDir {
        fn drop(&mut self) {
            for _ in 0..5 {
                match std::fs::remove_dir_all(&self.0) {
                    Ok(()) => return,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
                }
            }
        }
    }

    #[test]
    fn resolve_workspace_root_rejects_file_and_missing() {
        let missing = resolve_workspace_root("D:/anvil-missing-workspace-root");
        assert!(missing.unwrap_err().contains("not a directory"));
    }

    #[test]
    fn relative_workspace_root_resolves_to_the_same_nonempty_lane_as_absolute() {
        let current = std::env::current_dir().unwrap();
        let relative = JobPaths::from_resolved(
            resolve_workspace_root(".").expect("relative root"),
            std::env::temp_dir().join("anvil-relative-home"),
        );
        let absolute = JobPaths::from_resolved(
            resolve_workspace_root(&current.display().to_string()).expect("absolute root"),
            std::env::temp_dir().join("anvil-relative-home"),
        );
        assert_eq!(relative.dir, absolute.dir);
        assert_ne!(
            relative.dir.parent().and_then(|path| path.file_name()),
            Some(std::ffi::OsStr::new("workspaces"))
        );
    }

    #[test]
    fn bank_is_under_memory_lane_not_workspace() {
        let workspace = TempTestDir::new("job-ws");
        let home = TempTestDir::new("job-home");
        let paths = JobPaths::from_resolved(workspace.path().into(), home.path().into());
        assert!(paths.dir.starts_with(home.path()));
        assert!(paths.dir.ends_with("anvil"));
        assert!(paths.dir.to_string_lossy().contains("memories"));
        assert!(paths.dir.to_string_lossy().contains("workspaces"));
        assert!(!paths.dir.starts_with(workspace.path()));
    }

    #[test]
    fn legacy_anvil_bank_is_copied_to_the_canonical_lane() {
        let workspace = TempTestDir::new("job-migrate-ws");
        let home = TempTestDir::new("job-migrate-home");
        let legacy_slug =
            crate::utility::system_map::sanitize_key(&workspace.path().to_string_lossy());
        let legacy = home
            .path()
            .join("memories")
            .join("workspaces")
            .join(legacy_slug)
            .join("anvil");
        std::fs::create_dir_all(&legacy).expect("legacy lane");
        std::fs::write(legacy.join("anvil.lock.json"), "{}\n").expect("legacy lock");

        let paths = JobPaths::from_resolved(workspace.path().into(), home.path().into());
        assert_eq!(
            paths.dir.parent().and_then(Path::file_name),
            Some(std::ffi::OsStr::new(&workspace_key(
                &workspace.path().to_string_lossy()
            )))
        );
        assert!(paths.lock_path().is_file());
        assert!(legacy.join("anvil.lock.json").is_file());
    }

    #[test]
    fn active_jobs_summary_reports_lock_and_report_state() {
        let workspace = TempTestDir::new("sum-ws");
        let home = TempTestDir::new("sum-home");
        let paths = JobPaths::from_resolved(workspace.path().into(), home.path().into());

        // No lock yet: the axis must be empty, never fabricated.
        assert!(active_jobs_summary(home.path(), Some(workspace.path())).is_empty());

        paths.ensure_dir().unwrap();
        std::fs::write(paths.lock_path(), "{}").unwrap();
        let summary = active_jobs_summary(home.path(), Some(workspace.path()));
        let lane_id = workspace_key(&workspace.path().to_string_lossy());
        assert_eq!(summary, vec![(lane_id.clone(), "active".to_string())]);

        std::fs::write(paths.report_path(), "{}").unwrap();
        let summary = active_jobs_summary(home.path(), Some(workspace.path()));
        assert_eq!(summary, vec![(lane_id, "complete".to_string())]);
    }

    #[test]
    fn active_jobs_summary_scans_all_lanes_when_unscoped() {
        let home = TempTestDir::new("sum-scan");
        let workspaces = home.path().join("memories").join("workspaces");
        for slug in ["b-slug", "a-slug"] {
            let lane = workspaces.join(slug).join("anvil");
            std::fs::create_dir_all(&lane).unwrap();
            std::fs::write(lane.join("anvil.lock.json"), "{}").unwrap();
        }
        // A lane without a lock is not a job.
        std::fs::create_dir_all(workspaces.join("empty-slug").join("anvil")).unwrap();

        let summary = active_jobs_summary(home.path(), None);
        assert_eq!(
            summary,
            vec![
                ("a-slug".to_string(), "active".to_string()),
                ("b-slug".to_string(), "active".to_string()),
            ]
        );
    }

    #[test]
    fn same_workspace_and_home_share_bank() {
        let workspace = TempTestDir::new("job-share");
        let home = TempTestDir::new("job-share-home");
        let other_home = TempTestDir::new("job-other-home");
        let first = JobPaths::from_resolved(workspace.path().into(), home.path().into());
        let second = JobPaths::from_resolved(workspace.path().into(), home.path().into());
        assert_eq!(first.dir, second.dir);
        let other = JobPaths::from_resolved(workspace.path().into(), other_home.path().into());
        assert_ne!(first.dir, other.dir);
    }

    #[test]
    fn pieces_from_lock_filters_and_rejects_unknown() {
        let lock = serde_json::json!({
            "pieces": [
                {"id": "main", "files": ["a.rs"], "gates": ["echo ok"], "critic": "none"},
                {"id": "parse", "files": [], "gates": ["echo ok"], "critic": "none"}
            ]
        });
        let only = pieces_from_lock(&lock, "parse").expect("piece");
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].id, "parse");
        let err = pieces_from_lock(&lock, "nope").unwrap_err();
        assert!(err.contains("not in lock"));
        let empty = pieces_from_lock(&serde_json::json!({"pieces": []}), "").unwrap_err();
        assert!(empty.contains("empty"));
    }

    #[test]
    fn n_casts_clamps() {
        let lock = serde_json::json!({"budget": {"n_casts": 99}});
        assert_eq!(n_casts(&lock), 8);
        let lock = serde_json::json!({"budget": {"n_casts": 0}});
        assert_eq!(n_casts(&lock), 1);
        let lock = serde_json::json!({});
        assert_eq!(n_casts(&lock), 3);
    }

    #[test]
    fn operation_lease_is_exclusive_and_released_by_drop() {
        let workspace = TempTestDir::new("lease-ws");
        let home = TempTestDir::new("lease-home");
        let paths = JobPaths::from_resolved(workspace.path().into(), home.path().into());
        let first = JobLease::acquire(&paths).expect("first lease");
        assert!(JobLease::acquire(&paths).is_err());
        drop(first);
        assert!(JobLease::acquire(&paths).is_ok());
    }
}
