//! Purpose: Git-backed workspace code checkpoints — the buildable analog to
//!   Claude Code's native `/rewind`. Snapshot the working tree non-destructively,
//!   list/show snapshots, and restore one (guarded), so a bad edit run is undoable.
//! Caller: commands.rs `checkpoint` dispatch.
//! Dependencies: std::io, crate::args, crate::json, crate::runtime::{run_command, resolve_repository_root}.
//! Side Effects: Creates git objects + refs under `refs/claude-checkpoints/`; `restore`
//!   modifies the working tree (guarded by --confirm and an auto safety snapshot).
//!
//! Why this exists: native `/rewind` auto-captures Claude's edit-tool changes and
//! can restore code+conversation. An external binary cannot hook the edit tool,
//! but git already IS the code-undo. `checkpoint create` uses `git stash create`
//! to capture tracked working-tree changes as a dangling commit, then pins it
//! under a `refs/claude-checkpoints/<id>` ref so it survives gc and is listable.
//! Restore is the only destructive verb and is gated behind `--confirm`, with an
//! automatic safety checkpoint of the current state taken first so restore itself
//! is reversible.

use std::io::Write;
use std::path::Path;

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::runtime::{resolve_repository_root, run_command};

const REF_PREFIX: &str = "refs/claude-checkpoints/";

pub fn run_checkpoint_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help(&arguments[0]) {
        render_help(standard_output);
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "create" => run_create(&arguments[1..], standard_output, standard_error),
        "list" => run_list(&arguments[1..], standard_output, standard_error),
        "show" => run_show(&arguments[1..], standard_output, standard_error),
        "restore" => run_restore(&arguments[1..], standard_output, standard_error),
        other => {
            let _ = writeln!(
                standard_error,
                "Unknown checkpoint command: {other} (expected create|list|show|restore)"
            );
            1
        }
    }
}

fn render_help(standard_output: &mut dyn Write) {
    let _ = writeln!(
        standard_output,
        "Usage: claude-skills checkpoint [create|list|show|restore] [flags]"
    );
    let _ = writeln!(
        standard_output,
        "  create [--label <text>] [--repo-root <path>] [--json]   snapshot tracked working-tree changes"
    );
    let _ = writeln!(
        standard_output,
        "  list [--repo-root <path>] [--json]                      list saved checkpoints"
    );
    let _ = writeln!(
        standard_output,
        "  show --id <id> [--repo-root <path>]                     show a checkpoint's diffstat"
    );
    let _ = writeln!(
        standard_output,
        "  restore --id <id> --confirm [--repo-root <path>]        apply a checkpoint to the working tree (destructive; takes a safety snapshot first)"
    );
}

fn resolve_root(
    flag_value: &str,
    label: &str,
    standard_error: &mut dyn Write,
) -> Option<std::path::PathBuf> {
    match resolve_repository_root(flag_value) {
        Ok(path) => Some(path),
        Err(error) => {
            let _ = writeln!(standard_error, "{label}: {error}");
            None
        }
    }
}

fn git(
    repository_root: &Path,
    args: &[&str],
    label: &str,
    standard_error: &mut dyn Write,
) -> Option<String> {
    let owned: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    match run_command("git", &owned, Some(repository_root)) {
        Ok(result) if result.code == 0 => {
            Some(String::from_utf8_lossy(&result.stdout).trim().to_string())
        }
        Ok(result) => {
            let _ = writeln!(
                standard_error,
                "{label}: git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&result.stderr).trim()
            );
            None
        }
        Err(error) => {
            let _ = writeln!(standard_error, "{label}: {error}");
            None
        }
    }
}

fn run_create(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("checkpoint create");
    flags.string_flag("label", "");
    flags.string_flag("repo-root", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let Some(root) = resolve_root(
        flags.string_value("repo-root"),
        "checkpoint create",
        standard_error,
    ) else {
        return 1;
    };
    let label = {
        let raw = flags.string_value("label").trim().to_string();
        if raw.is_empty() {
            "checkpoint".to_string()
        } else {
            raw
        }
    };
    match create_checkpoint(&root, &label, standard_error) {
        Some(id) => {
            if flags.bool_value("json") {
                let payload = Value::Object(vec![
                    ("created".into(), Value::Bool(true)),
                    ("id".into(), Value::String(id.clone())),
                    ("label".into(), Value::String(label)),
                ]);
                return render_json(standard_output, standard_error, &payload);
            }
            let _ = writeln!(standard_output, "checkpoint create: {id}");
            let _ = writeln!(
                standard_output,
                "  restore with: claude-skills checkpoint restore --id {id} --confirm"
            );
            0
        }
        None => 1,
    }
}

/// Snapshot tracked working-tree changes as a dangling commit (`git stash create`)
/// pinned under a checkpoint ref. Returns the checkpoint id, or None on failure or
/// when there are no changes to snapshot. `standard_error` carries any message.
fn create_checkpoint(root: &Path, label: &str, standard_error: &mut dyn Write) -> Option<String> {
    // `git stash create` writes a commit object capturing tracked changes WITHOUT
    // touching the working tree or the stash stack. Empty output = clean tree.
    let sha = git(
        root,
        &["stash", "create", label],
        "checkpoint create",
        standard_error,
    )?;
    if sha.is_empty() {
        let _ = writeln!(
            standard_error,
            "checkpoint create: no tracked changes to snapshot (working tree clean)"
        );
        return None;
    }
    let id = checkpoint_id();
    let ref_name = format!("{REF_PREFIX}{id}");
    git(
        root,
        &["update-ref", "-m", label, &ref_name, &sha],
        "checkpoint create",
        standard_error,
    )?;
    Some(id)
}

fn run_list(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("checkpoint list");
    flags.string_flag("repo-root", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let Some(root) = resolve_root(
        flags.string_value("repo-root"),
        "checkpoint list",
        standard_error,
    ) else {
        return 1;
    };
    let Some(raw) = git(
        &root,
        &[
            "for-each-ref",
            "--sort=-creatordate",
            "--format=%(refname:strip=2)\t%(creatordate:iso8601)\t%(subject)",
            REF_PREFIX,
        ],
        "checkpoint list",
        standard_error,
    ) else {
        return 1;
    };
    let entries: Vec<(String, String, String)> = raw
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut fields = line.splitn(3, '\t');
            (
                fields.next().unwrap_or_default().to_string(),
                fields.next().unwrap_or_default().to_string(),
                fields.next().unwrap_or_default().to_string(),
            )
        })
        .collect();
    if flags.bool_value("json") {
        let payload = Value::Object(vec![
            ("count".into(), Value::Number(entries.len().to_string())),
            (
                "checkpoints".into(),
                Value::Array(
                    entries
                        .iter()
                        .map(|(id, created, label)| {
                            Value::Object(vec![
                                ("id".into(), Value::String(id.clone())),
                                ("createdAt".into(), Value::String(created.clone())),
                                ("label".into(), Value::String(label.clone())),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]);
        return render_json(standard_output, standard_error, &payload);
    }
    let _ = writeln!(
        standard_output,
        "checkpoint list: {} checkpoint(s)",
        entries.len()
    );
    for (id, created, label) in &entries {
        let _ = writeln!(standard_output, "  {id}  {created}  {label}");
    }
    if entries.is_empty() {
        let _ = writeln!(
            standard_output,
            "  none yet — create one with: claude-skills checkpoint create"
        );
    }
    0
}

fn run_show(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("checkpoint show");
    flags.string_flag("id", "");
    flags.string_flag("repo-root", "");
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let id = flags.string_value("id").trim().to_string();
    if id.is_empty() {
        let _ = writeln!(standard_error, "checkpoint show: --id is required");
        return 1;
    }
    let Some(root) = resolve_root(
        flags.string_value("repo-root"),
        "checkpoint show",
        standard_error,
    ) else {
        return 1;
    };
    let ref_name = format!("{REF_PREFIX}{id}");
    if !checkpoint_exists(&root, &id) {
        let _ = writeln!(
            standard_error,
            "checkpoint show: no checkpoint with id {id}"
        );
        return 1;
    }
    // `<ref>^!` shows the stash commit's own diff (against its first parent).
    let Some(stat) = git(
        &root,
        &["show", "--stat", "--oneline", &format!("{ref_name}^!")],
        "checkpoint show",
        standard_error,
    ) else {
        return 1;
    };
    let _ = writeln!(standard_output, "checkpoint {id}:");
    let _ = writeln!(standard_output, "{stat}");
    0
}

fn run_restore(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("checkpoint restore");
    flags.string_flag("id", "");
    flags.string_flag("repo-root", "");
    flags.bool_flag("confirm", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let id = flags.string_value("id").trim().to_string();
    if id.is_empty() {
        let _ = writeln!(standard_error, "checkpoint restore: --id is required");
        return 1;
    }
    let Some(root) = resolve_root(
        flags.string_value("repo-root"),
        "checkpoint restore",
        standard_error,
    ) else {
        return 1;
    };
    if !checkpoint_exists(&root, &id) {
        let _ = writeln!(
            standard_error,
            "checkpoint restore: no checkpoint with id {id}"
        );
        return 1;
    }
    // Destructive: applying overwrites tracked files in the working tree. Require
    // explicit --confirm so an accidental restore cannot clobber uncommitted work.
    if !flags.bool_value("confirm") {
        let _ = writeln!(
            standard_error,
            "checkpoint restore: this overwrites tracked files in the working tree. Re-run with --confirm to proceed.\n  A safety snapshot of the current state is taken automatically before restore, so the restore itself is reversible."
        );
        return 1;
    }
    // Take a safety snapshot of the CURRENT state first so restore is reversible.
    // If the tree is clean this is a no-op (None) and we proceed.
    let mut safety_sink: Vec<u8> = Vec::new();
    let safety_id = create_checkpoint(&root, &format!("pre-restore-of-{id}"), &mut safety_sink);

    let ref_name = format!("{REF_PREFIX}{id}");
    let Some(_) = git(
        &root,
        &["stash", "apply", &ref_name],
        "checkpoint restore",
        standard_error,
    ) else {
        return 1;
    };
    let _ = writeln!(standard_output, "checkpoint restore: applied {id}");
    match safety_id {
        Some(safety) => {
            let _ = writeln!(
                standard_output,
                "  pre-restore safety snapshot: {safety} (undo this restore with: claude-skills checkpoint restore --id {safety} --confirm)"
            );
        }
        None => {
            let _ = writeln!(
                standard_output,
                "  (working tree was clean before restore; no safety snapshot needed)"
            );
        }
    }
    0
}

fn checkpoint_exists(root: &Path, id: &str) -> bool {
    let ref_name = format!("{REF_PREFIX}{id}");
    run_command(
        "git",
        &[
            "show-ref".to_string(),
            "--verify".to_string(),
            "--quiet".to_string(),
            ref_name,
        ],
        Some(root),
    )
    .map(|result| result.code == 0)
    .unwrap_or(false)
}

fn checkpoint_id() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    format!("ckpt-{millis:x}")
}

fn render_json(
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
    value: &Value,
) -> u8 {
    if let Err(error) = write_indented(standard_output, value) {
        let _ = writeln!(standard_error, "checkpoint: render JSON: {error}");
        return 1;
    }
    0
}

fn is_help(argument: &str) -> bool {
    matches!(argument, "help" | "--help" | "-h")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn unique_repo(label: &str) -> PathBuf {
        let unique: u128 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let pid = std::process::id();
        std::env::temp_dir().join(format!("claude-skills-ckpt-{label}-{pid}-{unique}"))
    }

    fn git_init(root: &Path) {
        std::fs::create_dir_all(root).unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
            vec!["commit", "--allow-empty", "-q", "-m", "base"],
        ] {
            let owned: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            run_command("git", &owned, Some(root)).expect("git setup");
        }
    }

    #[test]
    fn create_list_show_restore_round_trip() {
        let root = unique_repo("roundtrip");
        git_init(&root);
        // Track a file and commit it so changes are "tracked working-tree changes".
        std::fs::write(root.join("a.txt"), "original\n").unwrap();
        run_command(
            "git",
            &["add", "a.txt"]
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
            Some(&root),
        )
        .unwrap();
        run_command(
            "git",
            &["commit", "-q", "-m", "add a"]
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
            Some(&root),
        )
        .unwrap();

        // Modify the file, then checkpoint the change.
        std::fs::write(root.join("a.txt"), "modified\n").unwrap();
        let mut err = Vec::new();
        let id = create_checkpoint(&root, "wip", &mut err)
            .unwrap_or_else(|| panic!("create failed: {}", String::from_utf8_lossy(&err)));
        assert!(checkpoint_exists(&root, &id));

        // Revert the file to original on disk (simulating a bad edit we want to undo).
        std::fs::write(root.join("a.txt"), "original\n").unwrap();

        // Restore the checkpoint and confirm the modification comes back.
        let restore_args: Vec<String> = vec![
            "restore".into(),
            "--id".into(),
            id.clone(),
            "--confirm".into(),
            "--repo-root".into(),
            root.to_string_lossy().to_string(),
        ];
        let mut out = Vec::new();
        let mut rerr = Vec::new();
        let code = run_checkpoint_command(&restore_args, &mut out, &mut rerr);
        assert_eq!(
            code,
            0,
            "restore stderr: {}",
            String::from_utf8_lossy(&rerr)
        );
        let restored = std::fs::read_to_string(root.join("a.txt")).unwrap();
        // Trim trailing whitespace so the assertion is line-ending agnostic
        // (git may materialize CRLF on Windows checkouts).
        assert_eq!(
            restored.trim_end(),
            "modified",
            "checkpoint did not restore the change"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn create_on_clean_tree_reports_nothing_to_snapshot() {
        let root = unique_repo("clean");
        git_init(&root);
        let mut err = Vec::new();
        let result = create_checkpoint(&root, "x", &mut err);
        assert!(result.is_none());
        assert!(String::from_utf8_lossy(&err).contains("no tracked changes"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_without_confirm_refuses() {
        let root = unique_repo("noconfirm");
        git_init(&root);
        std::fs::write(root.join("a.txt"), "x\n").unwrap();
        run_command(
            "git",
            &["add", "a.txt", "-A"]
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
            Some(&root),
        )
        .unwrap();
        run_command(
            "git",
            &["commit", "-q", "-m", "c"]
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
            Some(&root),
        )
        .unwrap();
        std::fs::write(root.join("a.txt"), "y\n").unwrap();
        let mut err = Vec::new();
        let id = create_checkpoint(&root, "wip", &mut err).expect("create");

        let args: Vec<String> = vec![
            "restore".into(),
            "--id".into(),
            id,
            "--repo-root".into(),
            root.to_string_lossy().to_string(),
        ];
        let mut out = Vec::new();
        let mut rerr = Vec::new();
        let code = run_checkpoint_command(&args, &mut out, &mut rerr);
        assert_eq!(code, 1);
        assert!(String::from_utf8_lossy(&rerr).contains("--confirm"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn show_and_restore_reject_unknown_id() {
        let root = unique_repo("unknown");
        git_init(&root);
        let root_arg = root.to_string_lossy().to_string();
        for verb in ["show", "restore"] {
            let args: Vec<String> = vec![
                verb.into(),
                "--id".into(),
                "ckpt-doesnotexist".into(),
                "--repo-root".into(),
                root_arg.clone(),
            ];
            let mut out = Vec::new();
            let mut err = Vec::new();
            let code = run_checkpoint_command(&args, &mut out, &mut err);
            assert_eq!(code, 1, "{verb} should fail on unknown id");
            assert!(String::from_utf8_lossy(&err).contains("no checkpoint with id"));
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
