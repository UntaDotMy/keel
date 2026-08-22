//! Purpose: `observe` — a single local session-health surface that composes the
//!   axes the existing analytics commands do NOT cover. `gain` and `session`
//!   already report the token-savings axis thoroughly; this command surfaces
//!   memory/recall health, working-brief presence, and real anvil job state in
//!   one read, and points at `gain`/`session` for tokens rather than re-parsing
//!   the compaction event log.
//! Caller: commands.rs `observe` dispatch.
//! Dependencies: the recall, anvil job-store, and working_brief utility
//!   surfaces, plus the shared args/json/runtime helpers.
//! Side Effects: read-only. `recall_status_snapshot` opens (and lazily syncs)
//!   the recall index; everything else reads files. No writes, no network.
//!
//! Design: reuse existing command data so `observe` cannot drift from its
//! source surfaces; this command is an aggregating read, not a second source.

use std::io::Write;
use std::path::PathBuf;

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::runtime::{display_path, resolve_claude_home};
use crate::utility::anvil::job::active_jobs_summary;
use crate::utility::recall::recall_status_snapshot;
use crate::utility::working_brief::list_briefs;

pub fn run_observe_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("observe");
    flag_set.bool_flag("json", false);
    flag_set.string_flag("claude-home", "");
    flag_set.string_flag("workspace-root", "");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }

    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(home) => home,
        Err(error) => {
            let _ = writeln!(standard_error, "observe: {error}");
            return 1;
        }
    };

    // Workspace root defaults to the current directory (the key the anvil job
    // store is slugged on), matching how the anvil surface resolves it.
    let workspace_root = {
        let flag = flag_set.string_value("workspace-root").trim().to_string();
        if flag.is_empty() {
            std::env::current_dir()
                .map(|path| display_path(&path))
                .unwrap_or_else(|_| ".".to_string())
        } else {
            display_path(&PathBuf::from(flag))
        }
    };

    let observation = collect_observation(&claude_home, &workspace_root);

    if flag_set.bool_value("json") {
        return write_indented(standard_output, &observation.to_json()).map_or(1, |_| 0);
    }
    observation.render_text(standard_output);
    0
}

/// Aggregated, read-only snapshot of session health across the non-token axes.
/// Each field is `Result`-flavored at collection time but flattened to a
/// display-ready shape here so a partial failure (e.g. the recall index could
/// not open) degrades to a clear note rather than failing the whole command —
/// observability must not go dark because one source had a hiccup.
struct Observation {
    /// (document_count, last_indexed_at_ms) or an error note.
    recall: Result<(u64, u128), String>,
    /// (brief count, most recent request line) or an error note.
    briefs: Result<(usize, Option<String>), String>,
    /// (id, state) per anvil job in this workspace; empty means none was ever run,
    /// and renderers omit the axis instead of fabricating a placeholder.
    anvil_jobs: Vec<(String, String)>,
    workspace_root: String,
}

fn collect_observation(claude_home: &std::path::Path, workspace_root: &str) -> Observation {
    let recall = recall_status_snapshot(claude_home)
        .map(|snapshot| (snapshot.document_count, snapshot.last_indexed_at_millis));

    let briefs = list_briefs(claude_home)
        .map_err(|error| error.to_string())
        .map(|briefs| {
            // Briefs are listed oldest-first, so the most recent is the last entry.
            let most_recent = briefs.last().map(|brief| brief.request.clone());
            (briefs.len(), most_recent)
        });

    let anvil_jobs = active_jobs_summary(claude_home, Some(std::path::Path::new(workspace_root)));
    Observation {
        recall,
        briefs,
        anvil_jobs,
        workspace_root: workspace_root.to_string(),
    }
}

impl Observation {
    fn to_json(&self) -> Value {
        let recall_value = match &self.recall {
            Ok((documents, last_indexed)) => Value::Object(vec![
                ("documents".into(), Value::Number(documents.to_string())),
                (
                    "lastIndexedAtMillis".into(),
                    Value::Number(last_indexed.to_string()),
                ),
            ]),
            Err(error) => Value::Object(vec![("error".into(), Value::String(error.clone()))]),
        };
        let briefs_value = match &self.briefs {
            Ok((count, most_recent)) => Value::Object(vec![
                ("count".into(), Value::Number(count.to_string())),
                (
                    "mostRecentRequest".into(),
                    match most_recent {
                        Some(request) => Value::String(request.clone()),
                        // json::Value has no Null variant; an empty string reads
                        // cleanly as "no brief recorded yet" for this surface.
                        None => Value::String(String::new()),
                    },
                ),
            ]),
            Err(error) => Value::Object(vec![("error".into(), Value::String(error.clone()))]),
        };
        let mut fields = vec![
            (
                "workspaceRoot".into(),
                Value::String(self.workspace_root.clone()),
            ),
            ("memory".into(), recall_value),
            ("workingBriefs".into(), briefs_value),
        ];
        // Honest axis: omit the anvil key entirely when nothing was ever run.
        if !self.anvil_jobs.is_empty() {
            let jobs = self
                .anvil_jobs
                .iter()
                .map(|(id, state)| {
                    Value::Object(vec![
                        ("id".into(), Value::String(id.clone())),
                        ("state".into(), Value::String(state.clone())),
                    ])
                })
                .collect::<Vec<_>>();
            fields.push(("anvilJobs".into(), Value::Array(jobs)));
        }
        fields.push((
            "tokenAnalytics".into(),
            Value::String("see `keel gain` and `keel session` for the token-savings axis".into()),
        ));
        Value::Object(fields)
    }

    fn render_text(&self, standard_output: &mut dyn Write) {
        let _ = writeln!(standard_output, "keel observe: session health");
        let _ = writeln!(standard_output, "workspace: {}", self.workspace_root);

        match &self.recall {
            Ok((documents, _)) => {
                let _ = writeln!(standard_output, "memory: {documents} document(s) indexed");
            }
            Err(error) => {
                let _ = writeln!(standard_output, "memory: unavailable ({error})");
            }
        }

        match &self.briefs {
            Ok((count, most_recent)) => {
                let _ = writeln!(standard_output, "working briefs: {count}");
                if let Some(request) = most_recent {
                    let trimmed = truncate_for_display(request, 80);
                    let _ = writeln!(standard_output, "  latest: {trimmed}");
                }
            }
            Err(error) => {
                let _ = writeln!(standard_output, "working briefs: unavailable ({error})");
            }
        }
        // Omit the anvil axis when no jobs are active.
        // Do not print a fabricated placeholder.
        if !self.anvil_jobs.is_empty() {
            for (id, state) in &self.anvil_jobs {
                let _ = writeln!(standard_output, "anvil: {id} [{state}]");
            }
        }

        let _ = writeln!(
            standard_output,
            "tokens: run `keel gain` / `keel session` for the savings axis"
        );
    }
}

/// Truncate a string to `max` characters with an ellipsis, counting by chars
/// (not bytes) so a multi-byte request line never slices mid-codepoint.
fn truncate_for_display(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let prefix: String = text.chars().take(max).collect();
        format!("{prefix}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ENV_LOCK;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tempdir_under(label: &str) -> PathBuf {
        let unique: u128 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let candidate = std::env::temp_dir().join(format!("{label}-{unique}"));
        fs::create_dir_all(&candidate).expect("create tempdir");
        candidate
    }

    #[test]
    fn observe_json_reports_all_axes_for_empty_home() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = tempdir_under("keel-observe-empty").join("home");
        fs::create_dir_all(&home).expect("create home");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let code = run_observe_command(
            &[
                "--json".to_string(),
                "--claude-home".to_string(),
                home.to_string_lossy().to_string(),
                "--workspace-root".to_string(),
                home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let rendered = String::from_utf8_lossy(&stdout);
        // Memory and briefs are always present; the anvil axis is omitted on an
        // empty home instead of reporting a fabricated inactive placeholder.
        assert!(rendered.contains("\"memory\""), "rendered: {rendered}");
        assert!(
            rendered.contains("\"workingBriefs\""),
            "rendered: {rendered}"
        );
        assert!(!rendered.contains("\"anvil\""), "rendered: {rendered}");
        assert!(!rendered.contains("\"active\""), "rendered: {rendered}");
        let _ = fs::remove_dir_all(home.parent().unwrap());
    }

    #[test]
    fn observe_text_omits_anvil_axis_when_nothing_ran_and_points_to_gain() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = tempdir_under("keel-observe-text").join("home");
        fs::create_dir_all(&home).expect("create home");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let code = run_observe_command(
            &[
                "--claude-home".to_string(),
                home.to_string_lossy().to_string(),
                "--workspace-root".to_string(),
                home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let rendered = String::from_utf8_lossy(&stdout);
        assert!(rendered.contains("memory:"), "rendered: {rendered}");
        assert!(rendered.contains("working briefs:"), "rendered: {rendered}");
        // Honest axis: no fabricated "none active" line on an empty workspace.
        assert!(!rendered.contains("anvil:"), "rendered: {rendered}");
        // Observability must point at the token axis it deliberately does not
        // duplicate, so a user knows where to look.
        assert!(
            rendered.contains("keel gain"),
            "observe must point at gain for the token axis; rendered: {rendered}"
        );
        let _ = fs::remove_dir_all(home.parent().unwrap());
    }

    #[test]
    fn observe_reports_seeded_anvil_job() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = tempdir_under("keel-observe-seeded");
        let home = root.join("home");
        fs::create_dir_all(&home).expect("create home");
        // Seed the anvil job store for this exact workspace slug.
        let slug = crate::utility::system_map::sanitize_key(&home.to_string_lossy());
        let lane = home
            .join("memories")
            .join("workspaces")
            .join(&slug)
            .join("anvil");
        fs::create_dir_all(&lane).expect("seed lane");
        fs::write(lane.join("anvil.lock.json"), "{}").expect("seed lock");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let code = run_observe_command(
            &[
                "--json".to_string(),
                "--claude-home".to_string(),
                home.to_string_lossy().to_string(),
                "--workspace-root".to_string(),
                home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let rendered = String::from_utf8_lossy(&stdout);
        assert!(
            rendered.contains("\"anvilJobs\""),
            "seeded job must appear; rendered: {rendered}"
        );
        assert!(rendered.contains("\"active\""), "rendered: {rendered}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn truncate_for_display_counts_chars_not_bytes() {
        assert_eq!(truncate_for_display("short", 80), "short");
        let long = "x".repeat(100);
        let truncated = truncate_for_display(&long, 80);
        assert_eq!(truncated.chars().count(), 81); // 80 + ellipsis
    }
}
