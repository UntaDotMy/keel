//! Purpose: Top-level command dispatch and command implementations for the Rust-native keel surface.
//! Caller: keel binary entrypoint (`main.rs`).
//! Dependencies: args, help, json, manager, review, runner, runtime, utility modules and the keel-platform/releaseassets crates.
//! Main Functions: Application::new, Application::effective_build_version, Application::run.
//! Side Effects: Parses CLI flags, detects platform target, writes formatted output to stdout/stderr, and dispatches only Rust-native command handlers.

use std::env;
use std::io::Write;
use std::path::PathBuf;

use keel_flow::{
    load_check, new_template_check, resolve_artifact_path, validate_check, write_check,
    DEFAULT_ARTIFACT_PATH, SCHEMA_VERSION,
};
use keel_platform::{detect_current_target, Target};
use keel_releaseassets::{
    archive_file_name, cache_directory, executable_path, normalize_release_tag,
    release_download_url, DEFAULT_REPOSITORY_SLUG,
};

use crate::args::FlagSet;
use crate::help::render_help_surface;
use crate::json::{write_indented, Value};
use crate::runtime::clean_path;
use crate::{manager, mcp, review, runner, utility};

const FOUNDATION_PHASE_NAME: &str = "phase-1-foundation";

pub struct Application {
    pub build_version: String,
    pub default_repository_slug: String,
}

impl Application {
    pub fn new(build_version: &str) -> Self {
        Self {
            build_version: build_version.to_string(),
            default_repository_slug: DEFAULT_REPOSITORY_SLUG.to_string(),
        }
    }

    pub fn effective_build_version(&self) -> String {
        if self.build_version.is_empty() {
            "dev".to_string()
        } else {
            self.build_version.clone()
        }
    }

    pub fn run(
        &self,
        arguments: &[String],
        standard_output: &mut dyn Write,
        standard_error: &mut dyn Write,
    ) -> u8 {
        if arguments.is_empty() {
            let _ = writeln!(
                standard_output,
                "keel - discipline as code. Run `keel help` for commands or `keel status` for install state."
            );
            return 0;
        }
        let command_name = arguments[0].trim().to_string();
        let command_arguments = &arguments[1..];
        match command_name.as_str() {
            "help" | "--help" | "-h" => {
                self.run_help_command(command_arguments, standard_output, standard_error)
            }
            "version" => {
                self.run_version_command(command_arguments, standard_output, standard_error)
            }
            "platform" => {
                self.run_platform_command(command_arguments, standard_output, standard_error)
            }
            "bootstrap-info" => {
                self.run_bootstrap_info_command(command_arguments, standard_output, standard_error)
            }
            "install" => manager::run_install_command(
                &self.effective_build_version(),
                command_arguments,
                standard_output,
                standard_error,
            ),
            "update" => manager::run_update_command(
                &self.effective_build_version(),
                command_arguments,
                standard_output,
                standard_error,
            ),
            "status" | "st" => manager::run_status_command(
                &self.effective_build_version(),
                command_arguments,
                standard_output,
                standard_error,
            ),
            "doctor" => manager::run_doctor_command(
                &self.effective_build_version(),
                command_arguments,
                standard_output,
                standard_error,
            ),
            "repair" => {
                manager::run_repair_command(command_arguments, standard_output, standard_error)
            }
            "verify" | "v" => {
                manager::run_verify_command(command_arguments, standard_output, standard_error)
            }
            "uninstall" | "remove" => {
                manager::run_uninstall_command(command_arguments, standard_output, standard_error)
            }
            "validate" => {
                manager::run_validate_command(command_arguments, standard_output, standard_error)
            }
            "all" => manager::run_all_command(
                &self.effective_build_version(),
                command_arguments,
                standard_output,
                standard_error,
            ),
            "menu" => manager::run_menu_command(standard_output),
            "review" => {
                review::run_review_command(command_arguments, standard_output, standard_error)
            }
            "git-workflow" => {
                review::run_git_workflow_command(command_arguments, standard_output, standard_error)
            }
            "run" => runner::run_run_command(command_arguments, standard_output, standard_error),
            "rewrite" => {
                runner::run_rewrite_command(command_arguments, standard_output, standard_error)
            }
            "hook" => runner::run_hook_command(command_arguments, standard_output, standard_error),
            "bridge" => {
                runner::run_bridge_command(command_arguments, standard_output, standard_error)
            }
            "learn" => {
                runner::run_learn_command(command_arguments, standard_output, standard_error)
            }
            "raw" => runner::run_raw_command(command_arguments, standard_output, standard_error),
            "replay" => {
                runner::run_replay_command(command_arguments, standard_output, standard_error)
            }
            "code-search" => {
                utility::run_code_search_command(command_arguments, standard_output, standard_error)
            }
            "code-graph" => {
                utility::run_code_graph_command(command_arguments, standard_output, standard_error)
            }
            "user-story" => {
                utility::run_user_story_command(command_arguments, standard_output, standard_error)
            }
            "sprint" => {
                utility::run_sprint_command(command_arguments, standard_output, standard_error)
            }
            "work" => utility::run_work_command(command_arguments, standard_output, standard_error),
            "skill-lint" => {
                utility::run_skill_lint_command(command_arguments, standard_output, standard_error)
            }
            "skill-eval" => {
                utility::run_skill_eval_command(command_arguments, standard_output, standard_error)
            }
            "config-audit" => utility::run_config_audit_command(
                command_arguments,
                standard_output,
                standard_error,
            ),
            "checkpoint" => {
                utility::run_checkpoint_command(command_arguments, standard_output, standard_error)
            }
            "design-intelligence" => utility::run_design_intelligence_command(
                command_arguments,
                standard_output,
                standard_error,
            ),
            "memory" => utility::run_memory_command(
                "memory",
                command_arguments,
                standard_output,
                standard_error,
            ),
            "orchestration" => utility::run_orchestration_command(
                command_arguments,
                standard_output,
                standard_error,
            ),
            "workflow" => {
                utility::run_workflow_command(command_arguments, standard_output, standard_error)
            }
            "gain" => utility::run_gain_command(command_arguments, standard_output, standard_error),
            "session" => {
                utility::run_session_command(command_arguments, standard_output, standard_error)
            }
            "stats" => {
                utility::run_stats_command(command_arguments, standard_output, standard_error)
            }
            "bench" => {
                utility::run_bench_command(command_arguments, standard_output, standard_error)
            }
            "eval" => utility::run_eval_command(command_arguments, standard_output, standard_error),
            "observe" => {
                utility::run_observe_command(command_arguments, standard_output, standard_error)
            }
            "dispatch" => {
                utility::run_dispatch_command(command_arguments, standard_output, standard_error)
            }
            "team" => utility::run_team_command(command_arguments, standard_output, standard_error),
            "flow" => self.run_flow_command(command_arguments, standard_output, standard_error),
            "telemetry" => {
                runner::run_telemetry_command(command_arguments, standard_output, standard_error)
            }
            "mcp" => mcp::run_mcp_command(command_arguments, standard_output, standard_error),
            "__self-replace" => {
                manager::run_self_replace_command(command_arguments, standard_error)
            }
            _ => {
                let _ = writeln!(
                    standard_error,
                    "Unknown command: {command_name}. The Rust runtime has no Go fallback."
                );
                let _ = render_help_surface(standard_error, false);
                1
            }
        }
    }

    fn run_help_command(
        &self,
        arguments: &[String],
        standard_output: &mut dyn Write,
        standard_error: &mut dyn Write,
    ) -> u8 {
        if arguments.is_empty() {
            let _ = render_help_surface(standard_output, false);
            return 0;
        }
        if arguments.len() == 1 && arguments[0].trim() == "advanced" {
            let _ = render_help_surface(standard_output, true);
            return 0;
        }
        let _ = writeln!(standard_error, "Usage: help [advanced]");
        1
    }

    fn run_version_command(
        &self,
        arguments: &[String],
        standard_output: &mut dyn Write,
        standard_error: &mut dyn Write,
    ) -> u8 {
        let mut flag_set = FlagSet::new("version");
        flag_set.bool_flag("json", false);
        if let Err(parse_error) = flag_set.parse(arguments) {
            let _ = writeln!(standard_error, "{}", parse_error.message);
            return 1;
        }
        let target = match detect_current_target() {
            Ok(target) => target,
            Err(target_error) => {
                let _ = writeln!(
                    standard_error,
                    "Unable to detect current target: {target_error}"
                );
                return 1;
            }
        };
        let effective_build_version = self.effective_build_version();
        if flag_set.bool_value("json") {
            let payload = Value::Object(vec![
                (
                    "buildVersion".into(),
                    Value::String(effective_build_version.clone()),
                ),
                (
                    "foundation".into(),
                    Value::String(FOUNDATION_PHASE_NAME.into()),
                ),
                ("target".into(), target_value(&target)),
                (
                    "featureSet".into(),
                    Value::String(Self::build_feature_set().into()),
                ),
            ]);
            return render_json(standard_output, standard_error, &payload);
        }
        let _ = writeln!(standard_output, "keel version: {effective_build_version}");
        let _ = writeln!(
            standard_output,
            "native foundation: {FOUNDATION_PHASE_NAME}"
        );
        let _ = writeln!(standard_output, "target: {}", target.directory_name());
        let _ = writeln!(
            standard_output,
            "feature set: {}",
            Self::build_feature_set()
        );
        0
    }

    /// Compile-time feature set of THIS binary: `semantic` when built with the
    /// `semantic` feature (vector recall via the embedded BERT model), else
    /// `standard`. Surfaced in `version` so operators can tell which binary is
    /// installed — the silent semantic<->standard swap was a real support trap.
    fn build_feature_set() -> &'static str {
        if cfg!(feature = "semantic") {
            "semantic"
        } else {
            "standard"
        }
    }

    fn run_platform_command(
        &self,
        arguments: &[String],
        standard_output: &mut dyn Write,
        standard_error: &mut dyn Write,
    ) -> u8 {
        let mut flag_set = FlagSet::new("platform");
        flag_set.bool_flag("json", false);
        if let Err(parse_error) = flag_set.parse(arguments) {
            let _ = writeln!(standard_error, "{}", parse_error.message);
            return 1;
        }
        let target = match detect_current_target() {
            Ok(target) => target,
            Err(target_error) => {
                let _ = writeln!(
                    standard_error,
                    "Unable to detect current target: {target_error}"
                );
                return 1;
            }
        };
        if flag_set.bool_value("json") {
            let payload = target_value(&target);
            return render_json(standard_output, standard_error, &payload);
        }
        let _ = writeln!(standard_output, "{}", target.directory_name());
        0
    }

    fn run_bootstrap_info_command(
        &self,
        arguments: &[String],
        standard_output: &mut dyn Write,
        standard_error: &mut dyn Write,
    ) -> u8 {
        let mut flag_set = FlagSet::new("bootstrap-info");
        flag_set.bool_flag("json", false);
        flag_set.string_flag("claude-home", "");
        flag_set.string_flag("repository-slug", self.default_repository_slug.clone());
        flag_set.string_flag("repo-root", "");
        flag_set.string_flag("version", self.effective_build_version());
        if let Err(parse_error) = flag_set.parse(arguments) {
            let _ = writeln!(standard_error, "{}", parse_error.message);
            return 1;
        }
        let target = match detect_current_target() {
            Ok(target) => target,
            Err(target_error) => {
                let _ = writeln!(
                    standard_error,
                    "Unable to detect current target: {target_error}"
                );
                return 1;
            }
        };
        let requested_claude_home = flag_set.string_value("claude-home").trim().to_string();
        let effective_claude_home = if requested_claude_home.is_empty() {
            match resolve_default_claude_home_directory() {
                Ok(resolved) => resolved,
                Err(resolve_error) => {
                    let _ = writeln!(
                        standard_error,
                        "Unable to resolve default harness home: {resolve_error}"
                    );
                    return 1;
                }
            }
        } else {
            requested_claude_home
        };
        let repository_slug_flag = flag_set.string_value("repository-slug").to_string();
        let repository_slug_trimmed = repository_slug_flag.trim().to_string();
        let repository_root_trimmed = flag_set.string_value("repo-root").trim().to_string();
        let build_version_flag = flag_set.string_value("version").to_string();
        let build_version_trimmed = build_version_flag.trim().to_string();

        let release_tag = normalize_release_tag(&build_version_flag);
        let cache_dir = cache_directory(&effective_claude_home, &build_version_flag, &target);
        let exec_path = executable_path(&effective_claude_home, &build_version_flag, &target);
        let archive = archive_file_name(&build_version_flag, &target);
        let download_url =
            release_download_url(&repository_slug_flag, &build_version_flag, &target);

        if flag_set.bool_value("json") {
            let mut object_fields = vec![
                (
                    "buildVersion".into(),
                    Value::String(build_version_trimmed.clone()),
                ),
                ("releaseTag".into(), Value::String(release_tag.clone())),
                (
                    "foundation".into(),
                    Value::String(FOUNDATION_PHASE_NAME.into()),
                ),
                ("target".into(), target_value(&target)),
                (
                    "repositorySlug".into(),
                    Value::String(repository_slug_trimmed.clone()),
                ),
            ];
            if !repository_root_trimmed.is_empty() {
                object_fields.push((
                    "repositoryRoot".into(),
                    Value::String(repository_root_trimmed.clone()),
                ));
            }
            object_fields.push((
                "claudeHomeDirectory".into(),
                Value::String(effective_claude_home.clone()),
            ));
            object_fields.push((
                "cacheDirectory".into(),
                Value::String(path_to_display_string(&cache_dir)),
            ));
            object_fields.push((
                "executablePath".into(),
                Value::String(path_to_display_string(&exec_path)),
            ));
            object_fields.push(("archiveFileName".into(), Value::String(archive.clone())));
            object_fields.push((
                "releaseDownloadUrl".into(),
                Value::String(download_url.clone()),
            ));
            return render_json(
                standard_output,
                standard_error,
                &Value::Object(object_fields),
            );
        }
        let _ = writeln!(standard_output, "build version: {build_version_trimmed}");
        let _ = writeln!(standard_output, "release tag: {release_tag}");
        let _ = writeln!(
            standard_output,
            "native foundation: {FOUNDATION_PHASE_NAME}"
        );
        let _ = writeln!(standard_output, "target: {}", target.directory_name());
        let _ = writeln!(
            standard_output,
            "repository slug: {repository_slug_trimmed}"
        );
        if !repository_root_trimmed.is_empty() {
            let _ = writeln!(
                standard_output,
                "repository root: {repository_root_trimmed}"
            );
        }
        let _ = writeln!(standard_output, "claude home: {effective_claude_home}");
        let _ = writeln!(
            standard_output,
            "cache directory: {}",
            path_to_display_string(&cache_dir)
        );
        let _ = writeln!(
            standard_output,
            "executable path: {}",
            path_to_display_string(&exec_path)
        );
        let _ = writeln!(standard_output, "archive file: {archive}");
        let _ = writeln!(standard_output, "release download url: {download_url}");
        0
    }

    fn run_flow_command(
        &self,
        arguments: &[String],
        standard_output: &mut dyn Write,
        standard_error: &mut dyn Write,
    ) -> u8 {
        if arguments.is_empty() {
            render_flow_help(standard_output);
            return 0;
        }
        let subcommand_name = arguments[0].trim();
        let subcommand_arguments = &arguments[1..];
        match subcommand_name {
            "start" => {
                self.run_flow_start_command(subcommand_arguments, standard_output, standard_error)
            }
            "check" => self.run_flow_validate_command(
                subcommand_arguments,
                standard_output,
                standard_error,
            ),
            "finish" => self.run_flow_validate_command(
                subcommand_arguments,
                standard_output,
                standard_error,
            ),
            "help" | "--help" | "-h" => {
                render_flow_help(standard_output);
                0
            }
            _ => {
                let _ = writeln!(standard_error, "Unknown flow subcommand: {subcommand_name}");
                render_flow_help(standard_output);
                1
            }
        }
    }

    fn run_flow_start_command(
        &self,
        arguments: &[String],
        standard_output: &mut dyn Write,
        standard_error: &mut dyn Write,
    ) -> u8 {
        let mut flag_set = FlagSet::new("flow start");
        flag_set.string_flag("repo-root", ".");
        flag_set.string_flag("output", DEFAULT_ARTIFACT_PATH);
        flag_set.string_flag("target-file", "");
        flag_set.string_flag("target-function", "");
        flag_set.string_flag("task", "");
        flag_set.bool_flag("json", false);
        if let Err(parse_error) = flag_set.parse(arguments) {
            let _ = writeln!(standard_error, "{}", parse_error.message);
            return 1;
        }
        if flag_set.string_value("target-file").trim().is_empty() {
            let _ = writeln!(standard_error, "flow start requires --target-file");
            return 1;
        }

        let repository_root = resolve_flow_repository_root(flag_set.string_value("repo-root"));
        let mut check = new_template_check(
            flag_set.string_value("target-file"),
            flag_set.string_value("target-function"),
        );
        check.task = flag_set.string_value("task").trim().to_string();
        let artifact_path =
            match write_check(&repository_root, flag_set.string_value("output"), check) {
                Ok(path) => path,
                Err(write_error) => {
                    let _ = writeln!(
                        standard_error,
                        "Unable to write flow check artifact: {write_error}"
                    );
                    return 1;
                }
            };

        if flag_set.bool_value("json") {
            let payload = Value::Object(vec![
                ("created".into(), Value::Bool(true)),
                (
                    "path".into(),
                    Value::String(path_to_display_string(&artifact_path)),
                ),
                ("schema".into(), Value::Number(SCHEMA_VERSION.to_string())),
            ]);
            return render_json(standard_output, standard_error, &payload);
        }
        let _ = writeln!(
            standard_output,
            "Created flow check artifact at {}",
            path_to_display_string(&artifact_path)
        );
        let _ = writeln!(
            standard_output,
            "Fill the owner path and validation evidence before editing existing source files."
        );
        0
    }

    fn run_flow_validate_command(
        &self,
        arguments: &[String],
        standard_output: &mut dyn Write,
        standard_error: &mut dyn Write,
    ) -> u8 {
        let mut flag_set = FlagSet::new("flow check");
        flag_set.string_flag("repo-root", ".");
        flag_set.string_flag("artifact", DEFAULT_ARTIFACT_PATH);
        flag_set.bool_flag("json", false);
        if let Err(parse_error) = flag_set.parse(arguments) {
            let _ = writeln!(standard_error, "{}", parse_error.message);
            return 1;
        }
        let repository_root = resolve_flow_repository_root(flag_set.string_value("repo-root"));
        let artifact_path =
            resolve_artifact_path(&repository_root, flag_set.string_value("artifact"));
        let check = match load_check(&repository_root, flag_set.string_value("artifact")) {
            Ok(check) => check,
            Err(load_error) => {
                if flag_set.bool_value("json") {
                    let payload = Value::Object(vec![
                        (
                            "errors".into(),
                            Value::Array(vec![Value::String(load_error.to_string())]),
                        ),
                        (
                            "path".into(),
                            Value::String(path_to_display_string(&artifact_path)),
                        ),
                        ("valid".into(), Value::Bool(false)),
                    ]);
                    return render_json(standard_output, standard_error, &payload).max(1);
                }
                let _ = writeln!(
                    standard_error,
                    "Flow check artifact is not valid: {load_error}"
                );
                return 1;
            }
        };

        let validation_errors = validate_check(check);
        if !validation_errors.is_empty() {
            if flag_set.bool_value("json") {
                let payload = Value::Object(vec![
                    (
                        "errors".into(),
                        Value::Array(validation_errors.into_iter().map(Value::String).collect()),
                    ),
                    (
                        "path".into(),
                        Value::String(path_to_display_string(&artifact_path)),
                    ),
                    ("valid".into(), Value::Bool(false)),
                ]);
                return render_json(standard_output, standard_error, &payload).max(1);
            }
            let _ = writeln!(
                standard_error,
                "Flow check artifact is incomplete at {}:",
                path_to_display_string(&artifact_path)
            );
            for validation_error in validation_errors {
                let _ = writeln!(standard_error, "  - {validation_error}");
            }
            return 1;
        }

        if flag_set.bool_value("json") {
            let payload = Value::Object(vec![
                (
                    "path".into(),
                    Value::String(path_to_display_string(&artifact_path)),
                ),
                ("schema".into(), Value::Number(SCHEMA_VERSION.to_string())),
                ("valid".into(), Value::Bool(true)),
            ]);
            return render_json(standard_output, standard_error, &payload);
        }
        let _ = writeln!(
            standard_output,
            "Flow check artifact is valid at {}",
            path_to_display_string(&artifact_path)
        );
        0
    }
}

fn target_value(target: &Target) -> Value {
    Value::Object(vec![
        (
            "operatingSystem".into(),
            Value::String(target.operating_system.clone()),
        ),
        (
            "architecture".into(),
            Value::String(target.architecture.clone()),
        ),
    ])
}

fn render_json(
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
    value: &Value,
) -> u8 {
    if let Err(write_error) = write_indented(standard_output, value) {
        let _ = writeln!(
            standard_error,
            "Unable to render JSON output: {write_error}"
        );
        return 1;
    }
    0
}

fn render_flow_help(standard_output: &mut dyn Write) {
    let _ = writeln!(
        standard_output,
        "Usage: keel flow [start|check|finish] [flags]"
    );
    let _ = writeln!(
        standard_output,
        "  flow start --target-file <path> [--target-function <name>] [--repo-root <path>] [--output <path>] [--json]"
    );
    let _ = writeln!(
        standard_output,
        "  Default artifact: ~/.claude/memories/workspaces/<workspace-slug>/flow/flow-check.json"
    );
    let _ = writeln!(
        standard_output,
        "  flow check [--repo-root <path>] [--artifact <path>] [--json]"
    );
    let _ = writeln!(
        standard_output,
        "  flow finish [--repo-root <path>] [--artifact <path>] [--json]"
    );
}

fn resolve_flow_repository_root(repository_root: &str) -> PathBuf {
    let mut trimmed_repository_root = repository_root.trim();
    if trimmed_repository_root.is_empty() {
        trimmed_repository_root = ".";
    }
    let candidate = PathBuf::from(trimmed_repository_root);
    let absolute_repository_root = if candidate.is_absolute() {
        candidate
    } else {
        match env::current_dir() {
            Ok(current_directory) => current_directory.join(candidate),
            Err(_) => PathBuf::from(".").join(candidate),
        }
    };
    clean_path(&absolute_repository_root)
}

fn resolve_default_claude_home_directory() -> Result<String, String> {
    // Mirror the CLI home resolution: KEEL_HOME, CLAUDE_TARGET_OVERRIDE,
    // ~/.keel when it exists, legacy ~/.claude otherwise.
    for env_name in ["KEEL_HOME", "CLAUDE_TARGET_OVERRIDE"] {
        if let Ok(override_value) = env::var(env_name) {
            let trimmed = override_value.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
    }
    match home_directory() {
        Some(user_home) => {
            let home_path = std::path::PathBuf::from(user_home);
            let keel_home = home_path.join(".keel");
            if keel_home.is_dir() {
                return Ok(path_to_display_string(&keel_home));
            }
            Ok(path_to_display_string(&home_path.join(".claude")))
        }
        None => Err("no user home directory available".to_string()),
    }
}

fn home_directory() -> Option<String> {
    if let Ok(home_value) = env::var("HOME") {
        if !home_value.is_empty() {
            return Some(home_value);
        }
    }
    if let Ok(userprofile_value) = env::var("USERPROFILE") {
        if !userprofile_value.is_empty() {
            return Some(userprofile_value);
        }
    }
    None
}

fn path_to_display_string(path: &std::path::Path) -> String {
    let rendered = path.to_string_lossy().to_string();
    if cfg!(windows) {
        rendered.replace('/', "\\")
    } else {
        rendered
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_flow::Check;
    use std::path::Path;

    #[test]
    fn flow_command_runs_start_check_finish_lifecycle() {
        let repository_root = tempdir_under("keel-flow-command-lifecycle");
        let artifact_path = repository_root.join("flow-check.json");
        let artifact = artifact_path.to_string_lossy().to_string();
        let application = Application::new("test-version");

        let mut start_stdout: Vec<u8> = Vec::new();
        let mut start_stderr: Vec<u8> = Vec::new();
        let start_code = application.run(
            &[
                "flow".to_string(),
                "start".to_string(),
                "--repo-root".to_string(),
                repository_root.to_string_lossy().to_string(),
                "--output".to_string(),
                artifact.clone(),
                "--target-file".to_string(),
                "rust/crates/keel/src/example.rs".to_string(),
                "--target-function".to_string(),
                "runExample".to_string(),
            ],
            &mut start_stdout,
            &mut start_stderr,
        );
        assert_eq!(
            start_code,
            0,
            "flow start stderr: {}",
            String::from_utf8_lossy(&start_stderr)
        );
        assert!(
            String::from_utf8_lossy(&start_stdout).contains("Created flow check artifact"),
            "unexpected start stdout: {}",
            String::from_utf8_lossy(&start_stdout)
        );

        let mut incomplete_stdout: Vec<u8> = Vec::new();
        let mut incomplete_stderr: Vec<u8> = Vec::new();
        let incomplete_code = application.run(
            &[
                "flow".to_string(),
                "check".to_string(),
                "--repo-root".to_string(),
                repository_root.to_string_lossy().to_string(),
                "--artifact".to_string(),
                artifact.clone(),
            ],
            &mut incomplete_stdout,
            &mut incomplete_stderr,
        );
        assert_ne!(incomplete_code, 0);
        assert!(
            String::from_utf8_lossy(&incomplete_stderr).contains("validation_evidence"),
            "unexpected incomplete stderr: {}",
            String::from_utf8_lossy(&incomplete_stderr)
        );

        write_complete_flow_check_for_tests(&repository_root, &artifact);
        let mut finish_stdout: Vec<u8> = Vec::new();
        let mut finish_stderr: Vec<u8> = Vec::new();
        let finish_code = application.run(
            &[
                "flow".to_string(),
                "finish".to_string(),
                "--repo-root".to_string(),
                repository_root.to_string_lossy().to_string(),
                "--artifact".to_string(),
                artifact.clone(),
                "--json".to_string(),
            ],
            &mut finish_stdout,
            &mut finish_stderr,
        );
        assert_eq!(
            finish_code,
            0,
            "flow finish stderr: {}",
            String::from_utf8_lossy(&finish_stderr)
        );
        assert!(
            String::from_utf8_lossy(&finish_stdout).contains("\"valid\": true"),
            "unexpected finish stdout: {}",
            String::from_utf8_lossy(&finish_stdout)
        );
        let _ = std::fs::remove_dir_all(&repository_root);
    }

    #[test]
    fn flow_command_renders_help_and_unknown_subcommand() {
        let application = Application::new("test-version");
        let mut help_stdout: Vec<u8> = Vec::new();
        let mut help_stderr: Vec<u8> = Vec::new();
        let help_code = application.run(&["flow".to_string()], &mut help_stdout, &mut help_stderr);
        assert_eq!(help_code, 0);
        assert!(help_stderr.is_empty());
        assert!(String::from_utf8_lossy(&help_stdout).contains("Usage: keel flow"));

        let mut unknown_stdout: Vec<u8> = Vec::new();
        let mut unknown_stderr: Vec<u8> = Vec::new();
        let unknown_code = application.run(
            &["flow".to_string(), "unknown".to_string()],
            &mut unknown_stdout,
            &mut unknown_stderr,
        );
        assert_eq!(unknown_code, 1);
        assert!(String::from_utf8_lossy(&unknown_stdout).contains("Usage: keel flow"));
        assert!(String::from_utf8_lossy(&unknown_stderr).contains("Unknown flow subcommand"));
    }

    #[test]
    fn unknown_top_level_command_fails_without_go_fallback() {
        let application = Application::new("test-version");
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = application.run(&["missing-command".to_string()], &mut stdout, &mut stderr);
        assert_eq!(exit_code, 1);
        assert!(stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&stderr).contains("no Go fallback"),
            "unexpected stderr: {}",
            String::from_utf8_lossy(&stderr)
        );
    }

    fn write_complete_flow_check_for_tests(repository_root: &Path, artifact_path: &str) {
        let complete_check = Check {
            version: SCHEMA_VERSION,
            target_file: "rust/crates/keel/src/example.rs".to_string(),
            target_function: "runExample".to_string(),
            current_behavior: "Existing example behavior remains stable.".to_string(),
            entry_point: "Application::run".to_string(),
            producer: "runExample".to_string(),
            source_of_truth: "rust/crates/keel/src/example.rs".to_string(),
            storage_state_queue_owner: "Not found".to_string(),
            side_effect_owner: "standard output".to_string(),
            consumers: vec!["CLI users".to_string()],
            cleanup_recovery_path: "exit code return".to_string(),
            edit_boundary: "rust/crates/keel/src/example.rs".to_string(),
            validation_needed: vec!["cargo test --workspace".to_string()],
            validation_evidence: vec!["cargo test --workspace passed".to_string()],
            ..Check::default()
        };
        write_check(repository_root, artifact_path, complete_check)
            .expect("write complete flow check");
    }

    fn tempdir_under(label: &str) -> PathBuf {
        let unique_suffix: u128 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let candidate = std::env::temp_dir().join(format!("{label}-{unique_suffix}"));
        std::fs::create_dir_all(&candidate).expect("create temporary directory");
        candidate
    }
}
