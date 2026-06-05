//! Purpose: Load optional project-specific declarative filters for the proxy.
//! Caller: proxy::adapters registry builder after built-in Rust adapters.
//! Dependencies: CommandAdapter contract and local TOML filter files.
//! Main Functions: load_project_filter_adapters.
//! Side Effects: Reads optional filter files from the current workspace.

use crate::adapters::common::{make_result, normalized_command};
use crate::proxy::adapter::{CommandAdapter, CompactResult};
use crate::proxy::command_ast::CommandAst;
use crate::proxy::raw_store::RunMeta;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
pub struct FilterConfig {
    #[serde(default)]
    pub filter: Vec<DeclarativeFilter>,
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
                        "[claude-skills] Warning: filter '{}' has an invalid regex command '{}': {} \
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

        let mut kept = Vec::new();
        for line in merged.lines() {
            let normalized = line.to_ascii_lowercase();

            // Apply remove filters first
            if self
                .filter
                .remove
                .iter()
                .any(|needle| normalized.contains(&needle.to_ascii_lowercase()))
            {
                continue;
            }

            // Apply keep filters
            if self.filter.keep.is_empty() {
                // If no keep filters specified, keep all non-removed lines
                kept.push(line.trim().to_string());
            } else if self
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
                    "[claude-skills] Warning: failed to parse filter file {}: {}",
                    path.display(),
                    error
                );
            }
        }
    }
    adapters
}

fn filter_paths() -> Vec<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_default();
    vec![
        cwd.join(".claude-skills").join("filters.toml"),
        cwd.join("claude-skills.filters.toml"),
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
}
