use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;

use crate::runtime::resolve_claude_home;
use crate::utility::anvil::lock::validate_lock;
use crate::utility::system_map::sanitize_key;

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
        let slug = sanitize_key(&workspace.to_string_lossy());
        let dir = home
            .join("memories")
            .join("workspaces")
            .join(slug)
            .join("anvil");
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
}

pub fn resolve_workspace_root(flag: &str) -> Result<PathBuf, String> {
    let path = if flag.trim().is_empty() {
        std::env::current_dir().map_err(|error| format!("workspace-root: {error}"))?
    } else {
        PathBuf::from(flag)
    };
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
/// Returns an empty vec when nothing was ever run — callers must omit the
/// axis rather than print a fabricated none-active placeholder.
pub fn active_jobs_summary(
    claude_home: &Path,
    workspace_root: Option<&Path>,
) -> Vec<(String, String)> {
    let workspaces = claude_home.join("memories").join("workspaces");
    let lanes: Vec<PathBuf> = match workspace_root {
        Some(root) => vec![workspaces
            .join(crate::utility::system_map::sanitize_key(
                &root.to_string_lossy(),
            ))
            .join("anvil")],
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

    #[test]
    fn resolve_workspace_root_rejects_file_and_missing() {
        let missing = resolve_workspace_root("D:/anvil-missing-workspace-root");
        assert!(missing.unwrap_err().contains("not a directory"));
    }

    #[test]
    fn bank_is_under_memory_lane_not_workspace() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("anvil-job-ws-{stamp}"));
        let home = std::env::temp_dir().join(format!("anvil-job-home-{stamp}"));
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let paths = JobPaths::from_resolved(workspace.clone(), home.clone());
        assert!(paths.dir.starts_with(&home));
        assert!(paths.dir.ends_with("anvil"));
        assert!(paths.dir.to_string_lossy().contains("memories"));
        assert!(paths.dir.to_string_lossy().contains("workspaces"));
        assert!(!paths.dir.starts_with(&workspace));
        let _ = std::fs::remove_dir_all(&workspace);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn active_jobs_summary_reports_lock_and_report_state() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("anvil-sum-ws-{stamp}"));
        let home = std::env::temp_dir().join(format!("anvil-sum-home-{stamp}"));
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let paths = JobPaths::from_resolved(workspace.clone(), home.clone());

        // No lock yet: the axis must be empty, never fabricated.
        assert!(active_jobs_summary(&home, Some(&workspace)).is_empty());

        paths.ensure_dir().unwrap();
        std::fs::write(paths.lock_path(), "{}").unwrap();
        let summary = active_jobs_summary(&home, Some(&workspace));
        let lane_id = sanitize_key(&workspace.to_string_lossy());
        assert_eq!(summary, vec![(lane_id.clone(), "active".to_string())]);

        std::fs::write(paths.report_path(), "{}").unwrap();
        let summary = active_jobs_summary(&home, Some(&workspace));
        assert_eq!(summary, vec![(lane_id, "complete".to_string())]);
        let _ = std::fs::remove_dir_all(&workspace);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn active_jobs_summary_scans_all_lanes_when_unscoped() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = std::env::temp_dir().join(format!("anvil-sum-scan-{stamp}"));
        let workspaces = home.join("memories").join("workspaces");
        for slug in ["b-slug", "a-slug"] {
            let lane = workspaces.join(slug).join("anvil");
            std::fs::create_dir_all(&lane).unwrap();
            std::fs::write(lane.join("anvil.lock.json"), "{}").unwrap();
        }
        // A lane without a lock is not a job.
        std::fs::create_dir_all(workspaces.join("empty-slug").join("anvil")).unwrap();

        let summary = active_jobs_summary(&home, None);
        assert_eq!(
            summary,
            vec![
                ("a-slug".to_string(), "active".to_string()),
                ("b-slug".to_string(), "active".to_string()),
            ]
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn same_workspace_and_home_share_bank() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("anvil-job-share-{stamp}"));
        let home = std::env::temp_dir().join(format!("anvil-job-share-home-{stamp}"));
        std::fs::create_dir_all(&workspace).unwrap();
        let first = JobPaths::from_resolved(workspace.clone(), home.clone());
        let second = JobPaths::from_resolved(workspace.clone(), home.clone());
        assert_eq!(first.dir, second.dir);
        let other = JobPaths::from_resolved(
            workspace.clone(),
            std::env::temp_dir().join(format!("anvil-job-other-home-{stamp}")),
        );
        assert_ne!(first.dir, other.dir);
        let _ = std::fs::remove_dir_all(&workspace);
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
}
