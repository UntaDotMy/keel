//! Purpose: Load optional project-specific declarative filters for the proxy.
//! Caller: proxy::adapters registry builder after built-in Rust adapters.
//! Dependencies: CommandAdapter contract and local TOML filter files.
//! Main Functions: load_project_filter_adapters.
//! Side Effects: Reads optional filter files from the current workspace.

use crate::adapters::common::{
    compact_json_structure, dedup_lines, make_result, normalized_command, redact_possible_secret,
    strip_ansi_escape,
};
use crate::proxy::adapter::{CommandAdapter, CompactResult};
use crate::proxy::command_ast::CommandAst;
use crate::proxy::raw_store::RunMeta;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct FilterConfig {
    #[serde(default)]
    pub filter: Vec<DeclarativeFilter>,
    #[serde(default)]
    pub verification: VerificationConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct VerificationConfig {
    #[serde(default)]
    pub commands: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationReport {
    pub projects: usize,
    pub commands: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeclarativeFilter {
    pub name: String,
    pub command: String,
    #[serde(default = "default_match_mode")]
    pub match_mode: MatchMode,
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub keep: Vec<String>,
    #[serde(default)]
    pub remove: Vec<String>,
    #[serde(default = "default_max_lines")]
    pub max_lines: usize,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub stages: Vec<FilterStage>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum FilterStage {
    #[serde(rename = "strip")]
    Strip { patterns: Vec<String> },
    #[serde(rename = "keep")]
    Keep { patterns: Vec<String> },
    #[serde(rename = "dedup")]
    Dedup,
    #[serde(rename = "head_tail")]
    HeadTail { head: usize, tail: usize },
    #[serde(rename = "signal")]
    Signal { max_lines: usize },
    #[serde(rename = "json_structure")]
    JsonStructure,
    #[serde(rename = "redact")]
    Redact,
    #[serde(rename = "strip_ansi")]
    StripAnsi,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    StartsWith,
    Exact,
    Contains,
    Regex,
}

fn default_match_mode() -> MatchMode {
    MatchMode::StartsWith
}

fn default_max_lines() -> usize {
    40
}

fn default_enabled() -> bool {
    true
}

pub struct ProjectFilterAdapter {
    filter: DeclarativeFilter,
    regex: Option<regex::Regex>,
}

impl ProjectFilterAdapter {
    fn new(filter: DeclarativeFilter) -> Self {
        let regex = if matches!(filter.match_mode, MatchMode::Regex) {
            match regex::Regex::new(&filter.command) {
                Ok(compiled) => Some(compiled),
                Err(error) => {
                    // A regex filter whose pattern does not compile would
                    // otherwise silently match nothing — the filter looks
                    // configured but never fires. Surface the compile error so
                    // the author can fix the pattern instead of debugging a
                    // no-op filter.
                    eprintln!(
                        "[keel] Warning: filter '{}' has an invalid regex command '{}': {} \
                         (this filter is disabled)",
                        filter.name, filter.command, error
                    );
                    None
                }
            }
        } else {
            None
        };
        Self { filter, regex }
    }
}

impl CommandAdapter for ProjectFilterAdapter {
    fn name(&self) -> &'static str {
        "project-filter"
    }

    fn matches(&self, ast: &CommandAst) -> bool {
        if !self.filter.enabled {
            return false;
        }
        let normalized = normalized_command(&ast.program, &ast.args);
        match self.filter.match_mode {
            MatchMode::StartsWith => {
                ast.original_command.starts_with(&self.filter.command)
                    || normalized.starts_with(&self.filter.command)
                    || ast.program.eq_ignore_ascii_case(&self.filter.command)
                    || ast.program.ends_with(&self.filter.command)
            }
            MatchMode::Exact => {
                ast.original_command == self.filter.command
                    || normalized == self.filter.command
                    || ast.program == self.filter.command
            }
            MatchMode::Contains => {
                ast.original_command.contains(&self.filter.command)
                    || normalized.contains(&self.filter.command)
                    || ast.program.contains(&self.filter.command)
            }
            MatchMode::Regex => self
                .regex
                .as_ref()
                .map(|re| {
                    re.is_match(&ast.original_command)
                        || re.is_match(&normalized)
                        || re.is_match(&ast.program)
                })
                .unwrap_or(false),
        }
    }

    fn compact(
        &self,
        stdout: &[u8],
        stderr: &[u8],
        exit_code: i32,
        meta: &RunMeta,
    ) -> CompactResult {
        if matches!(self.filter.exit_code, Some(expected) if expected != exit_code) {
            return make_result(
                self.name(),
                normalized_command(&meta.program, &meta.args),
                String::from_utf8_lossy(stdout).to_string(),
                String::from_utf8_lossy(stderr).to_string(),
                exit_code,
                meta,
                false,
            );
        }

        let merged = format!(
            "{}\n{}",
            String::from_utf8_lossy(stdout),
            String::from_utf8_lossy(stderr)
        );

        if !self.filter.stages.is_empty() {
            let rendered = apply_stages(&merged, &self.filter.stages);
            return make_result(
                self.name(),
                format!(
                    "filter {} (staged: {})",
                    self.filter.name,
                    self.filter.stages.len()
                ),
                rendered,
                String::new(),
                exit_code,
                meta,
                true,
            );
        }

        let mut kept = Vec::new();
        for line in merged.lines() {
            let normalized = line.to_ascii_lowercase();

            if self
                .filter
                .remove
                .iter()
                .any(|needle| normalized.contains(&needle.to_ascii_lowercase()))
            {
                continue;
            }

            if self.filter.keep.is_empty()
                || self
                    .filter
                    .keep
                    .iter()
                    .any(|needle| normalized.contains(&needle.to_ascii_lowercase()))
            {
                kept.push(line.trim().to_string());
            }

            if kept.len() >= self.filter.max_lines.max(1) {
                break;
            }
        }

        let rendered = if kept.is_empty() {
            format!("filter {} matched; no keep lines found", self.filter.name)
        } else {
            kept.join("\n")
        };

        make_result(
            self.name(),
            format!("filter {}", self.filter.name),
            rendered,
            String::new(),
            exit_code,
            meta,
            true,
        )
    }
}

fn apply_stages(input: &str, stages: &[FilterStage]) -> String {
    let mut text = input.to_string();
    for stage in stages {
        text = match stage {
            FilterStage::StripAnsi => strip_ansi_escape(&text),
            FilterStage::Strip { patterns } => {
                let mut result = String::new();
                for line in text.lines() {
                    let normalized = line.to_ascii_lowercase();
                    if patterns
                        .iter()
                        .any(|p| normalized.contains(&p.to_ascii_lowercase()))
                    {
                        continue;
                    }
                    result.push_str(line);
                    result.push('\n');
                }
                result.trim_end().to_string()
            }
            FilterStage::Keep { patterns } => {
                let mut result = Vec::new();
                for line in text.lines() {
                    let normalized = line.to_ascii_lowercase();
                    if patterns
                        .iter()
                        .any(|p| normalized.contains(&p.to_ascii_lowercase()))
                    {
                        result.push(line.to_string());
                    }
                }
                result.join("\n")
            }
            FilterStage::Dedup => dedup_lines(&text),
            FilterStage::HeadTail { head, tail } => {
                let head = *head;
                let tail = *tail;
                let lines: Vec<&str> = text.lines().collect();
                if lines.len() <= head + tail {
                    text
                } else {
                    let omitted = lines.len() - head - tail;
                    format!(
                        "{}\n... omitted {omitted} lines ...\n{}",
                        lines[..head].join("\n"),
                        lines[lines.len() - tail..].join("\n")
                    )
                }
            }
            FilterStage::Signal { max_lines } => {
                let signals = signal_lines(&text, *max_lines);
                if signals.is_empty() {
                    text
                } else {
                    signals.join("\n")
                }
            }
            FilterStage::JsonStructure => compact_json_structure(&text),
            FilterStage::Redact => {
                let mut result = String::new();
                for line in text.lines() {
                    result.push_str(&redact_possible_secret(line));
                    result.push('\n');
                }
                result.trim_end().to_string()
            }
        };
    }
    text
}

fn signal_lines(text: &str, max_lines: usize) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut selected = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let normalized = trimmed.to_ascii_lowercase();
        let is_signal = [
            "error",
            "failed",
            "failure",
            "fatal",
            "panic",
            "exception",
            "traceback",
            "assert",
            "warning",
            "denied",
            "not found",
            "cannot",
            "undefined",
            "mismatched",
            "expected",
            "actual",
            "timeout",
            "timed out",
        ]
        .iter()
        .any(|needle| normalized.contains(needle));
        if is_signal && seen.insert(trimmed.to_string()) {
            selected.push(trimmed.to_string());
        }
        if selected.len() >= max_lines {
            break;
        }
    }
    selected
}

pub fn load_project_filter_adapters() -> Vec<Box<dyn CommandAdapter>> {
    let mut adapters: Vec<Box<dyn CommandAdapter>> = Vec::new();
    for path in filter_paths() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match toml::from_str::<FilterConfig>(&text) {
            Ok(config) => {
                for filter in config.filter {
                    adapters.push(Box::new(ProjectFilterAdapter::new(filter)));
                }
            }
            Err(error) => {
                eprintln!(
                    "[keel] Warning: failed to parse filter file {}: {}",
                    path.display(),
                    error
                );
            }
        }
    }
    adapters
}

pub fn verification_report(repository_root: &Path) -> Result<VerificationReport, String> {
    let mut commands = Vec::new();
    for path in filter_paths_at(repository_root) {
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("read verification config {}: {error}", path.display()))?;
        let config: FilterConfig = toml::from_str(&text)
            .map_err(|error| format!("parse verification config {}: {error}", path.display()))?;
        commands.extend(config.verification.commands);
    }
    commands.sort();
    commands.dedup();

    let projects = discover_dart_projects(repository_root)?;
    let mut warnings = Vec::new();
    for (path, is_flutter) in &projects {
        if *is_flutter {
            if !commands
                .iter()
                .any(|command| strict_flutter_command(command))
            {
                warnings.push(format!(
                    "{}: Flutter project lacks strict analyzer verification; add `flutter analyze --fatal-infos --fatal-warnings` to [verification].commands in keel.filters.toml",
                    path.display()
                ));
            }
        } else if !commands.iter().any(|command| strict_dart_command(command)) {
            warnings.push(format!(
                "{}: Dart project lacks machine analyzer verification; add `dart analyze --format=machine` to [verification].commands in keel.filters.toml",
                path.display()
            ));
        }
    }
    Ok(VerificationReport {
        projects: projects.len(),
        commands,
        warnings,
    })
}

fn strict_flutter_command(command: &str) -> bool {
    let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized = normalized.to_ascii_lowercase();
    normalized.contains("flutter analyze")
        && normalized.contains("--fatal-infos")
        && normalized.contains("--fatal-warnings")
}

fn strict_dart_command(command: &str) -> bool {
    let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized = normalized.to_ascii_lowercase();
    normalized.contains("dart analyze")
        && (normalized.contains("--format=machine") || normalized.contains("--format machine"))
}

fn discover_dart_projects(repository_root: &Path) -> Result<Vec<(PathBuf, bool)>, String> {
    fn visit(
        directory: &Path,
        depth: usize,
        projects: &mut Vec<(PathBuf, bool)>,
    ) -> Result<(), String> {
        if depth > 5 || projects.len() >= 128 {
            return Ok(());
        }
        let entries = std::fs::read_dir(directory)
            .map_err(|error| format!("scan project directory {}: {error}", directory.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("read project entry: {error}"))?;
            let file_type = entry
                .file_type()
                .map_err(|error| format!("read project entry type: {error}"))?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if file_type.is_file() && name == "pubspec.yaml" {
                let metadata = entry.metadata().map_err(|error| {
                    format!("read pubspec metadata {}: {error}", path.display())
                })?;
                if metadata.len() > 1_048_576 {
                    return Err(format!("pubspec {} exceeds 1048576 bytes", path.display()));
                }
                let body = std::fs::read_to_string(&path)
                    .map_err(|error| format!("read pubspec {}: {error}", path.display()))?;
                let lower = body.to_ascii_lowercase();
                let is_flutter = lower.contains("sdk: flutter")
                    || lower.lines().any(|line| line.trim() == "flutter:");
                projects.push((path, is_flutter));
            } else if file_type.is_dir()
                && !matches!(
                    name.as_ref(),
                    ".git" | ".dart_tool" | "build" | "node_modules" | "target" | ".keel"
                )
            {
                visit(&path, depth + 1, projects)?;
            }
        }
        Ok(())
    }

    let mut projects = Vec::new();
    visit(repository_root, 0, &mut projects)?;
    projects.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(projects)
}

fn filter_paths() -> Vec<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_default();
    filter_paths_at(&cwd)
}

fn filter_paths_at(root: &Path) -> Vec<PathBuf> {
    vec![
        root.join(".keel").join("filters.toml"),
        root.join("keel.filters.toml"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_toml_filter_config() {
        let toml = r#"
[[filter]]
name = "cargo-test"
command = "cargo test"
match_mode = "starts_with"
exit_code = 0
keep = ["FAILED", "error", "test result"]
max_lines = 50

[[filter]]
name = "eslint"
command = "eslint"
match_mode = "contains"
keep = ["error", "warning"]
remove = ["info"]
"#;

        let config: FilterConfig = toml::from_str(toml).expect("valid toml");
        assert_eq!(config.filter.len(), 2);
        assert_eq!(config.filter[0].name, "cargo-test");
        assert_eq!(config.filter[0].max_lines, 50);
        assert!(matches!(config.filter[0].match_mode, MatchMode::StartsWith));
        assert_eq!(config.filter[1].remove, vec!["info"]);
    }

    #[test]
    fn parse_regex_filter() {
        let toml = r#"
[[filter]]
name = "pytest"
command = "^pytest .*-v"
match_mode = "regex"
keep = ["FAILED", "PASSED", "ERROR"]
"#;

        let config: FilterConfig = toml::from_str(toml).expect("valid toml");
        assert_eq!(config.filter.len(), 1);
        assert!(matches!(config.filter[0].match_mode, MatchMode::Regex));
    }

    #[test]
    fn filter_matches_starts_with() {
        let filter = DeclarativeFilter {
            name: "test".to_string(),
            command: "cargo test".to_string(),
            match_mode: MatchMode::StartsWith,
            exit_code: None,
            keep: vec!["FAILED".to_string()],
            remove: vec![],
            max_lines: 40,
            enabled: true,
            stages: vec![],
        };
        let adapter = ProjectFilterAdapter::new(filter);

        let ast = CommandAst {
            program: "cargo".to_string(),
            args: vec!["test".to_string()],
            original_command: "cargo test".to_string(),
            has_shell_syntax: false,
            shell_wrapped: false,
            cwd: PathBuf::from("."),
            detected_kind: crate::proxy::command_ast::CommandKind::Unknown,
        };
        assert!(adapter.matches(&ast));
    }

    #[test]
    fn filter_compact_with_remove() {
        let filter = DeclarativeFilter {
            name: "test".to_string(),
            command: "cargo".to_string(),
            match_mode: MatchMode::StartsWith,
            exit_code: None,
            keep: vec!["keep".to_string()],
            remove: vec!["noise".to_string()],
            max_lines: 40,
            enabled: true,
            stages: vec![],
        };
        let adapter = ProjectFilterAdapter::new(filter);
        let meta = RunMeta {
            raw_id: "1".to_string(),
            command: "cargo test".to_string(),
            program: "cargo".to_string(),
            args: vec!["test".to_string()],
            cwd: PathBuf::from("."),
            started_at: 0,
            duration_ms: 0,
            exit_code: 0,
            adapter_name: "test".to_string(),
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

        let stdout = b"keep this line\nnoise remove this\nanother keep line\n";
        let result = adapter.compact(stdout, &[], 0, &meta);
        assert!(result.compacted);
        assert!(result.stdout.contains("keep this line"));
        assert!(result.stdout.contains("another keep line"));
        assert!(!result.stdout.contains("noise"));
    }

    #[test]
    fn parse_staged_filter_config() {
        let toml = r#"
[[filter]]
name = "cargo-test-staged"
command = "cargo test"
match_mode = "starts_with"

[[filter.stages]]
type = "strip_ansi"

[[filter.stages]]
type = "strip"
patterns = ["warning", "deprecated"]

[[filter.stages]]
type = "signal"
max_lines = 20

[[filter.stages]]
type = "head_tail"
head = 5
tail = 5
"#;
        let config: FilterConfig = toml::from_str(toml).expect("valid toml");
        assert_eq!(config.filter.len(), 1);
        assert_eq!(config.filter[0].stages.len(), 4);
        assert!(matches!(config.filter[0].stages[0], FilterStage::StripAnsi));
        assert!(matches!(
            config.filter[0].stages[1],
            FilterStage::Strip { .. }
        ));
        assert!(matches!(
            config.filter[0].stages[2],
            FilterStage::Signal { .. }
        ));
        assert!(matches!(
            config.filter[0].stages[3],
            FilterStage::HeadTail { .. }
        ));
    }

    #[test]
    fn staged_filter_compact_applies_pipeline_in_order() {
        let filter = DeclarativeFilter {
            name: "staged".to_string(),
            command: "cargo".to_string(),
            match_mode: MatchMode::StartsWith,
            exit_code: None,
            keep: vec![],
            remove: vec![],
            max_lines: 40,
            enabled: true,
            stages: vec![
                FilterStage::Strip {
                    patterns: vec!["noise".to_string()],
                },
                FilterStage::Keep {
                    patterns: vec!["error".to_string()],
                },
            ],
        };
        let adapter = ProjectFilterAdapter::new(filter);
        let meta = RunMeta {
            raw_id: "1".to_string(),
            command: "cargo test".to_string(),
            program: "cargo".to_string(),
            args: vec!["test".to_string()],
            cwd: PathBuf::from("."),
            started_at: 0,
            duration_ms: 0,
            exit_code: 0,
            adapter_name: "test".to_string(),
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

        let stdout = b"noise line\nerror line here\nnoise line 2\nerror line 2\n";
        let result = adapter.compact(stdout, &[], 0, &meta);
        assert!(result.compacted);
        assert!(result.stdout.contains("error line here"));
        assert!(result.stdout.contains("error line 2"));
        assert!(!result.stdout.contains("noise"));
        assert!(result.summary.contains("staged: 2"));
    }

    #[test]
    fn staged_filter_head_tail_compacts_long_output() {
        let filter = DeclarativeFilter {
            name: "headtail".to_string(),
            command: "cargo".to_string(),
            match_mode: MatchMode::StartsWith,
            exit_code: None,
            keep: vec![],
            remove: vec![],
            max_lines: 40,
            enabled: true,
            stages: vec![FilterStage::HeadTail { head: 2, tail: 2 }],
        };
        let adapter = ProjectFilterAdapter::new(filter);
        let meta = RunMeta {
            raw_id: "1".to_string(),
            command: "cargo".to_string(),
            program: "cargo".to_string(),
            args: vec![],
            cwd: PathBuf::from("."),
            started_at: 0,
            duration_ms: 0,
            exit_code: 0,
            adapter_name: "test".to_string(),
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

        let stdout: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let result = adapter.compact(stdout.as_bytes(), &[], 0, &meta);
        assert!(result.compacted);
        assert!(result.stdout.contains("line 0"));
        assert!(result.stdout.contains("line 1"));
        assert!(result.stdout.contains("omitted"));
    }

    #[test]
    fn staged_filter_dedup_collapses_repeats() {
        let filter = DeclarativeFilter {
            name: "dedup".to_string(),
            command: "cargo".to_string(),
            match_mode: MatchMode::StartsWith,
            exit_code: None,
            keep: vec![],
            remove: vec![],
            max_lines: 40,
            enabled: true,
            stages: vec![FilterStage::Dedup],
        };
        let adapter = ProjectFilterAdapter::new(filter);
        let meta = RunMeta {
            raw_id: "1".to_string(),
            command: "cargo".to_string(),
            program: "cargo".to_string(),
            args: vec![],
            cwd: PathBuf::from("."),
            started_at: 0,
            duration_ms: 0,
            exit_code: 0,
            adapter_name: "test".to_string(),
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

        let stdout = b"done\ndone\ndone\nfinished\n";
        let result = adapter.compact(stdout, &[], 0, &meta);
        assert!(result.compacted);
        assert!(result.stdout.contains("(3x)"));
        assert!(result.stdout.contains("finished"));
    }

    #[test]
    fn staged_filter_redact_masks_secrets() {
        let filter = DeclarativeFilter {
            name: "redact".to_string(),
            command: "cargo".to_string(),
            match_mode: MatchMode::StartsWith,
            exit_code: None,
            keep: vec![],
            remove: vec![],
            max_lines: 40,
            enabled: true,
            stages: vec![FilterStage::Redact],
        };
        let adapter = ProjectFilterAdapter::new(filter);
        let meta = RunMeta {
            raw_id: "1".to_string(),
            command: "cargo".to_string(),
            program: "cargo".to_string(),
            args: vec![],
            cwd: PathBuf::from("."),
            started_at: 0,
            duration_ms: 0,
            exit_code: 0,
            adapter_name: "test".to_string(),
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

        let stdout = b"DATABASE_URL=postgres://user:pass@host/db\nnormal line\n";
        let result = adapter.compact(stdout, &[], 0, &meta);
        assert!(result.compacted);
        assert!(result.stdout.contains("[redacted possible secret"));
        assert!(result.stdout.contains("normal line"));
    }

    #[test]
    fn staged_filter_json_structure_compacts_json() {
        let filter = DeclarativeFilter {
            name: "json".to_string(),
            command: "cargo".to_string(),
            match_mode: MatchMode::StartsWith,
            exit_code: None,
            keep: vec![],
            remove: vec![],
            max_lines: 40,
            enabled: true,
            stages: vec![FilterStage::JsonStructure],
        };
        let adapter = ProjectFilterAdapter::new(filter);
        let meta = RunMeta {
            raw_id: "1".to_string(),
            command: "cargo".to_string(),
            program: "cargo".to_string(),
            args: vec![],
            cwd: PathBuf::from("."),
            started_at: 0,
            duration_ms: 0,
            exit_code: 0,
            adapter_name: "test".to_string(),
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

        let stdout = br#"{"name": "foo", "version": "1.0.0", "count": 42}"#;
        let result = adapter.compact(stdout, &[], 0, &meta);
        assert!(result.compacted);
        assert!(result.stdout.contains("<str>"));
        assert!(result.stdout.contains("<num>"));
    }

    #[test]
    fn verification_section_parses_declared_commands() {
        let config: FilterConfig = toml::from_str(
            r#"
[verification]
commands = [
  "flutter analyze --fatal-infos --fatal-warnings",
  "dart analyze --format=machine"
]
"#,
        )
        .unwrap();
        assert_eq!(config.verification.commands.len(), 2);
    }

    #[test]
    fn flutter_project_reports_missing_strict_analyzer_without_rewriting_config() {
        let root = std::env::temp_dir().join(format!(
            "keel-filter-flutter-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("pubspec.yaml"),
            "name: warning_fixture\ndependencies:\n  flutter:\n    sdk: flutter\n",
        )
        .unwrap();
        std::fs::write(
            root.join("keel.filters.toml"),
            "[verification]\ncommands = [\"flutter test\"]\n",
        )
        .unwrap();
        let original = std::fs::read_to_string(root.join("keel.filters.toml")).unwrap();
        let report = verification_report(&root).unwrap();
        assert_eq!(report.projects, 1);
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("--fatal-infos --fatal-warnings"));
        assert_eq!(
            std::fs::read_to_string(root.join("keel.filters.toml")).unwrap(),
            original,
            "inspection must never rewrite the owner's command policy"
        );
        std::fs::write(
            root.join("keel.filters.toml"),
            "[verification]\ncommands = [\"flutter analyze --fatal-infos --fatal-warnings\"]\n",
        )
        .unwrap();
        assert!(verification_report(&root).unwrap().warnings.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn dart_project_requires_machine_analyzer_command() {
        let root = std::env::temp_dir().join(format!(
            "keel-filter-dart-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("pubspec.yaml"), "name: dart_fixture\n").unwrap();
        let report = verification_report(&root).unwrap();
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("dart analyze --format=machine"));
        let _ = std::fs::remove_dir_all(root);
    }
}
