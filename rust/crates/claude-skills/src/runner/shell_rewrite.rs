//! Purpose: Shell command rewriting, analysis, and compaction routing for claude-skills run.
//! Caller: runner/mod.rs for rewrite command and hook pre-tool-use logic.
//! Dependencies: std::env, std::path, crate::runtime::display_path.
//! Main Functions: rewrite_command_text, analyze_command_text, shell_words_and_operators.
//! Side Effects: None (pure analysis and string transformation).

use std::path::Path;

use crate::runtime::display_path;

pub struct RewriteDecision {
    pub rewritten_command: String,
    pub supported: bool,
    pub reason: String,
}

pub enum RewriteShell {
    Bash,
    PlatformDefault,
}

pub fn adapter_name_for_rewrite(command: &str) -> &'static str {
    let analysis = analyze_command_text(command);

    let Some(program) = analysis
        .effective_fields
        .first()
        .map(|value| command_base_name(value))
    else {
        return "none";
    };

    match program.as_str() {
        "cargo" => {
            if analysis
                .effective_fields
                .iter()
                .any(|arg| arg == "test" || arg == "nextest")
            {
                "tests"
            } else if analysis
                .effective_fields
                .iter()
                .any(|arg| matches!(arg.as_str(), "build" | "check" | "clippy"))
            {
                if analysis.effective_fields.iter().any(|arg| arg == "clippy") {
                    "lint"
                } else {
                    "build"
                }
            } else {
                "generic"
            }
        }

        "pytest" | "jest" | "vitest" | "playwright" => "tests",

        "deno" if analysis.effective_fields.iter().any(|arg| arg == "test") => "tests",

        "deno" if analysis.effective_fields.iter().any(|arg| arg == "lint") => "lint",

        "deno" if analysis.effective_fields.iter().any(|arg| arg == "fmt") => "build",

        "python"
            if analysis.effective_fields.get(1).map(String::as_str) == Some("-m")
                && analysis.effective_fields.get(2).map(String::as_str) == Some("pytest") =>
        {
            "tests"
        }

        "go" if analysis.effective_fields.iter().any(|arg| arg == "test") => "tests",

        "go" if analysis
            .effective_fields
            .iter()
            .any(|arg| matches!(arg.as_str(), "build" | "vet")) =>
        {
            "build"
        }

        "npm" | "pnpm" | "yarn" | "bun" => package_manager_adapter_name_for_rewrite(
            analysis.effective_fields.iter().skip(1).map(String::as_str),
        ),

        "npx" | "pnpx" | "dlx" => package_manager_adapter_name_for_rewrite(
            analysis.effective_fields.iter().skip(1).map(String::as_str),
        ),

        "git" | "gh" => "git",

        "rg" | "grep" => "search",

        "find" | "cat" | "sed" | "head" | "tail" | "ls" | "tree" | "jq" => "files",

        "ruff" | "eslint" | "mypy" | "biome" | "rubocop" | "flake8" | "golangci-lint" => "lint",

        "tsc" | "prettier" | "webpack" | "vite" | "next" | "black" | "isort" => "build",

        "rspec" | "phpunit" => "tests",

        "rake" if analysis.effective_fields.iter().any(|arg| arg == "test") => "tests",

        "rake" => "build",

        "bundle" | "composer"
            if analysis
                .effective_fields
                .iter()
                .any(|arg| ["install", "update", "add", "require"].contains(&arg.as_str())) =>
        {
            "build"
        }

        "bundle" | "composer" => "build",

        "nx" if analysis.effective_fields.iter().any(|arg| arg == "test") => "tests",

        "nx" if analysis.effective_fields.iter().any(|arg| arg == "lint") => "lint",

        "nx" if analysis.effective_fields.iter().any(|arg| arg == "build") => "build",

        "brew" | "apt" | "apt-get" => "build",

        "docker" | "kubectl" | "terraform" | "aws" => "logs",

        "dotnet" | "mvn" | "gradle"
            if analysis.effective_fields.iter().any(|arg| arg == "test") =>
        {
            "tests"
        }

        "dotnet" | "mvn" | "gradle"
            if analysis
                .effective_fields
                .iter()
                .any(|arg| arg == "build" || arg == "vet") =>
        {
            "build"
        }

        "make" => "build",

        "curl" | "wget" => "logs",

        "pip" | "pip3" => "build",

        "journalctl" | "systemctl" => "logs",

        _ => "generic",
    }
}

fn package_manager_adapter_name_for_rewrite<'a>(
    args: impl Iterator<Item = &'a str>,
) -> &'static str {
    let args: Vec<&str> = args.collect();

    if args
        .iter()
        .any(|arg| matches!(*arg, "test" | "jest" | "vitest") || arg.ends_with(":test"))
    {
        "tests"
    } else if args.contains(&"build")
        || args
            .iter()
            .any(|arg| matches!(*arg, "install" | "i" | "ci" | "add" | "update"))
    {
        "build"
    } else {
        "generic"
    }
}

pub fn rewrite_command_text(command: &str) -> RewriteDecision {
    rewrite_command_text_for_shell(command, RewriteShell::PlatformDefault)
}

pub fn rewrite_for_doctor(command: &str) -> String {
    rewrite_command_text(command).rewritten_command
}

pub fn rewrite_command_text_for_shell(command: &str, shell: RewriteShell) -> RewriteDecision {
    let trimmed = command.trim();

    if trimmed.is_empty() {
        return RewriteDecision {
            rewritten_command: String::new(),

            supported: false,

            reason: "empty command".into(),
        };
    }

    let analysis = analyze_command_text(trimmed);

    if is_already_compaction_wrapped(&analysis.effective_fields) {
        return RewriteDecision {
            rewritten_command: trimmed.to_string(),

            supported: false,

            reason: "command already uses claude-skills run".into(),
        };
    }

    if !is_supported_noisy_command(&analysis.effective_fields) {
        return RewriteDecision {
            rewritten_command: String::new(),

            supported: false,

            reason: "no native claude-skills compaction filter for command".into(),
        };
    }

    let runnable_command = if analysis.requires_shell_wrapper {
        format!("bash -lc {}", shell_quote(trimmed))
    } else {
        trimmed.to_string()
    };

    RewriteDecision {
        rewritten_command: format!("{} {runnable_command}", compaction_command_prefix(shell)),

        supported: true,

        reason: String::new(),
    }
}

pub struct CommandAnalysis {
    pub effective_fields: Vec<String>,

    pub requires_shell_wrapper: bool,
}

pub fn analyze_command_text(command: &str) -> CommandAnalysis {
    let segments = split_shell_segments(command);

    let words = segments.first().cloned().unwrap_or_default();

    let mut effective_fields = effective_command_fields(&words, 0);

    for segment in segments.iter().skip(1) {
        let candidate = effective_command_fields(segment, 0);

        if is_supported_noisy_command(&candidate) || is_already_compaction_wrapped(&candidate) {
            effective_fields = candidate;

            break;
        }
    }

    let first_effective = effective_fields
        .first()
        .map(|value| value.to_string())
        .unwrap_or_default();

    let first_word = words.first().cloned().unwrap_or_default();

    CommandAnalysis {
        effective_fields,

        requires_shell_wrapper: contains_shell_syntax(command)
            || words
                .first()
                .map(|value| is_env_assignment(value))
                .unwrap_or(false)
            || (!first_word.is_empty()
                && !first_effective.is_empty()
                && command_base_name(&first_word) != command_base_name(&first_effective)
                && !matches!(
                    command_base_name(&first_word).as_str(),
                    "env" | "time" | "command" | "exec"
                )),
    }
}

fn split_shell_segments(command: &str) -> Vec<Vec<String>> {
    let (words, operators) = shell_words_and_operators(command);

    if words.is_empty() {
        return Vec::new();
    }

    let mut segments = Vec::new();

    let mut current = Vec::new();

    let mut operator_index = 0usize;

    for word in words {
        if is_shell_separator_token(&word) {
            if !current.is_empty() {
                segments.push(current);

                current = Vec::new();
            }

            operator_index += 1;

            continue;
        }

        current.push(word);
    }

    if !current.is_empty() {
        segments.push(current);
    }

    if segments.is_empty() && !operators.is_empty() && operator_index == 0 {
        segments.push(Vec::new());
    }

    segments
}

pub fn shell_words_and_operators(command: &str) -> (Vec<String>, Vec<String>) {
    let mut words = Vec::new();

    let mut operators = Vec::new();

    let mut current = String::new();

    let mut quote: Option<char> = None;

    let mut escaped = false;

    let characters: Vec<char> = command.chars().collect();

    let mut index = 0usize;

    while index < characters.len() {
        let character = characters[index];

        if escaped {
            current.push(character);

            escaped = false;

            index += 1;

            continue;
        }

        if character == '\\' && backslash_starts_escape(quote, characters.get(index + 1).copied()) {
            escaped = true;

            index += 1;

            continue;
        }

        if let Some(quote_character) = quote {
            if character == quote_character {
                quote = None;
            } else {
                current.push(character);
            }

            index += 1;

            continue;
        }

        if character == '\'' || character == '"' {
            quote = Some(character);

            index += 1;

            continue;
        }

        if character.is_whitespace() {
            if !current.is_empty() {
                words.push(current.clone());

                current.clear();
            }

            index += 1;

            continue;
        }

        if is_shell_operator_character(character) {
            if !current.is_empty() {
                words.push(current.clone());

                current.clear();
            }

            let mut operator = character.to_string();

            if let Some(next) = characters.get(index + 1).copied() {
                if matches!((character, next), ('|', '|') | ('&', '&') | ('>', '>')) {
                    operator.push(next);

                    index += 1;
                }
            }

            operators.push(operator.clone());

            words.push(operator);

            index += 1;

            continue;
        }

        current.push(character);

        index += 1;
    }

    if escaped {
        current.push('\\');
    }

    if !current.is_empty() {
        words.push(current);
    }

    (words, operators)
}

fn backslash_starts_escape(quote: Option<char>, next: Option<char>) -> bool {
    match quote {
        Some('\'') => false,

        Some('"') => next
            .map(|character| matches!(character, '"' | '\\' | '$' | '`' | '\n'))
            .unwrap_or(false),

        _ => next
            .map(|character| {
                character.is_whitespace()
                    || matches!(
                        character,
                        '\'' | '"' | '\\' | '$' | '`' | '|' | '&' | ';' | '<' | '>' | '(' | ')'
                    )
            })
            .unwrap_or(false),
    }
}

fn is_shell_operator_character(character: char) -> bool {
    matches!(
        character,
        '|' | '&' | ';' | '<' | '>' | '\n' | '`' | '(' | ')'
    )
}

fn is_shell_separator_token(value: &str) -> bool {
    matches!(
        value,
        "|" | "||" | "&&" | ";" | "&" | "<" | ">" | ">>" | "\n" | "`" | "(" | ")"
    )
}

fn contains_shell_syntax(command: &str) -> bool {
    !shell_words_and_operators(command).1.is_empty()
}

fn effective_command_fields(words: &[String], depth: usize) -> Vec<String> {
    if depth > 3 {
        return normalize_command_fields(words);
    }

    let mut index = 0;

    while words
        .get(index)
        .map(|value| is_env_assignment(value))
        .unwrap_or(false)
    {
        index += 1;
    }

    let Some(command) = words.get(index).map(|value| command_base_name(value)) else {
        return Vec::new();
    };

    match command.as_str() {
        "env" => {
            index += 1;

            while let Some(value) = words.get(index) {
                if is_env_assignment(value) {
                    index += 1;
                } else if matches!(value.as_str(), "-u" | "--unset" | "-C" | "--chdir") {
                    index += 2;
                } else if value.starts_with("--ignore-environment")
                    || value == "-i"
                    || value.starts_with('-')
                {
                    index += 1;
                } else {
                    break;
                }
            }

            effective_command_fields(&words[index..], depth + 1)
        }

        "time" | "command" | "exec" | "nohup" => {
            effective_command_fields(&words[index + 1..], depth + 1)
        }

        "sudo" | "doas" | "nice" => {
            index += 1;

            while words
                .get(index)
                .map(|value| value.starts_with('-'))
                .unwrap_or(false)
            {
                index += 1;
            }

            effective_command_fields(&words[index..], depth + 1)
        }

        "bash" | "sh" | "zsh" => {
            for (shell_index, word) in words[index + 1..].iter().enumerate() {
                if word.contains('c') && word.starts_with('-') {
                    if let Some(shell_command) = words.get(index + 1 + shell_index + 1) {
                        let nested_segments = split_shell_segments(shell_command);

                        for segment in &nested_segments {
                            let candidate = effective_command_fields(segment, depth + 1);

                            if is_supported_noisy_command(&candidate)
                                || is_already_compaction_wrapped(&candidate)
                            {
                                return candidate;
                            }
                        }

                        let nested_words = nested_segments.first().cloned().unwrap_or_default();

                        return effective_command_fields(&nested_words, depth + 1);
                    }
                }
            }

            normalize_command_fields(&words[index..])
        }

        _ => normalize_command_fields(&words[index..]),
    }
}

fn normalize_command_fields(words: &[String]) -> Vec<String> {
    words
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect()
}

pub fn is_env_assignment(value: &str) -> bool {
    let Some((name, _)) = value.split_once('=') else {
        return false;
    };

    !name.is_empty()
        && name
            .chars()
            .all(|character| character == '_' || character.is_ascii_alphanumeric())
        && name
            .chars()
            .next()
            .map(|character| character == '_' || character.is_ascii_alphabetic())
            .unwrap_or(false)
}

pub fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }

    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub fn bash_command_for_executable_args(path: &Path, arguments: &str) -> String {
    format!("{} {arguments}", shell_quote(&display_path(path)))
}

pub fn platform_default_command_for_executable_args(path: &Path, arguments: &str) -> String {
    let displayed_path = display_path(path);

    if cfg!(windows) {
        format!("& {} {arguments}", powershell_quote(&displayed_path))
    } else {
        bash_command_for_executable_args(path, arguments)
    }
}

pub fn is_already_compaction_wrapped(fields: &[String]) -> bool {
    if fields.len() >= 2 && command_base_name(&fields[0]) == "claude-skills" && fields[1] == "run" {
        return true;
    }

    fields.len() >= 2
        && command_base_name(&fields[0]).starts_with("claude-skills")
        && fields[1] == "run"
}

pub fn is_supported_noisy_command(fields: &[String]) -> bool {
    let Some(command) = fields.first().map(|value| command_base_name(value)) else {
        return false;
    };

    if matches!(
        command.as_str(),
        "cargo"
            | "npm"
            | "pnpm"
            | "yarn"
            | "bun"
            | "npx"
            | "pnpx"
            | "dlx"
            | "pytest"
            | "ruff"
            | "eslint"
            | "tsc"
            | "vitest"
            | "jest"
            | "playwright"
            | "deno"
            | "gradle"
            | "mvn"
            | "make"
            | "go"
            | "dotnet"
            | "docker"
            | "kubectl"
            | "terraform"
            | "aws"
            | "gh"
            | "git"
            | "rg"
            | "grep"
            | "find"
            | "cat"
            | "head"
            | "tail"
            | "sed"
            | "ls"
            | "tree"
            | "jq"
            | "mypy"
            | "prettier"
            | "biome"
            | "curl"
            | "wget"
            | "pip"
            | "pip3"
            | "journalctl"
            | "systemctl"
            | "rake"
            | "rspec"
            | "rubocop"
            | "bundle"
            | "flake8"
            | "black"
            | "isort"
            | "poetry"
            | "webpack"
            | "vite"
            | "nx"
            | "next"
            | "golangci-lint"
            | "composer"
            | "phpunit"
            | "brew"
            | "apt"
            | "apt-get"
    ) {
        return true;
    }

    command == "python"
        && fields.get(1).map(String::as_str) == Some("-m")
        && fields.get(2).map(String::as_str) == Some("pytest")
}

pub fn command_base_name(command: &str) -> String {
    let normalized = command.replace('\\', "/");

    let base_name = normalized.rsplit('/').next().unwrap_or(command);

    base_name
        .trim_end_matches(".exe")
        .trim_end_matches(".cmd")
        .trim_end_matches(".bat")
        .to_string()
}

pub fn compaction_command_prefix(shell: RewriteShell) -> String {
    match std::env::current_exe() {
        Ok(path) => match shell {
            RewriteShell::Bash => bash_command_for_executable_args(&path, "run --"),

            RewriteShell::PlatformDefault => {
                platform_default_command_for_executable_args(&path, "run --")
            }
        },

        Err(_) => "claude-skills run --".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::hook_lifecycle::hook_command_for_executable_args;

    #[test]
    fn rewrite_only_wraps_supported_noisy_commands() {
        let cargo = rewrite_command_text("cargo test --workspace");

        assert!(cargo.supported);

        assert!(cargo
            .rewritten_command
            .contains(" run -- cargo test --workspace"));

        let wrapped = rewrite_command_text("claude-skills run -- cargo test --workspace");

        assert!(!wrapped.supported);

        assert_eq!(wrapped.reason, "command already uses claude-skills run");

        let quiet = rewrite_command_text("echo hello");

        assert!(!quiet.supported);

        assert_eq!(
            quiet.reason,
            "no native claude-skills compaction filter for command"
        );
    }

    #[test]
    fn rewrite_adapter_metadata_matches_package_manager_subcommands() {
        assert_eq!(adapter_name_for_rewrite("npm test"), "tests");

        assert_eq!(adapter_name_for_rewrite("pnpm vitest"), "tests");

        assert_eq!(adapter_name_for_rewrite("yarn unit:test"), "tests");

        assert_eq!(adapter_name_for_rewrite("npm run build"), "build");

        assert_eq!(adapter_name_for_rewrite("npm install"), "build");

        assert_eq!(adapter_name_for_rewrite("yarn add vite"), "build");

        assert_eq!(adapter_name_for_rewrite("npm publish"), "generic");
    }

    #[test]
    fn rewrite_adapter_metadata_distinguishes_build_and_lint() {
        assert_eq!(adapter_name_for_rewrite("cargo build"), "build");

        assert_eq!(adapter_name_for_rewrite("cargo clippy"), "lint");

        assert_eq!(adapter_name_for_rewrite("eslint ."), "lint");

        assert_eq!(adapter_name_for_rewrite("tsc --noEmit"), "build");
    }

    #[test]
    fn rewrite_understands_shell_wrappers_and_env_prefixes() {
        let env_prefixed = rewrite_command_text("RUST_LOG=debug cargo test --workspace");

        assert!(env_prefixed.supported);

        assert!(env_prefixed.rewritten_command.contains("bash -lc"));

        assert!(env_prefixed
            .rewritten_command
            .contains("RUST_LOG=debug cargo test --workspace"));

        let shell_wrapped = rewrite_command_text("bash -lc 'cargo test --workspace'");

        assert!(shell_wrapped.supported);

        assert!(shell_wrapped.rewritten_command.contains("bash -lc"));

        assert!(shell_wrapped
            .rewritten_command
            .contains("cargo test --workspace"));

        let pipeline = rewrite_command_text("cargo test --workspace | tee test.log");

        assert!(pipeline.supported);

        assert!(pipeline.rewritten_command.contains("bash -lc"));
    }

    #[test]
    fn rewrite_parser_handles_late_supported_segments_and_windows_paths() {
        let late_supported = rewrite_command_text("printf ok | grep ok && cargo test --workspace");

        assert!(late_supported.supported);

        assert!(late_supported.rewritten_command.contains("bash -lc"));

        assert!(late_supported
            .rewritten_command
            .contains("cargo test --workspace"));

        let windows_path = rewrite_command_text(r#""C:\tools\cargo.exe" test --workspace"#);

        assert!(windows_path.supported);

        assert!(windows_path
            .rewritten_command
            .contains(r#""C:\tools\cargo.exe" test --workspace"#));

        let already_wrapped =
            rewrite_command_text(r#""C:\Users\me\.claude\claude-skills.exe" run -- cargo test"#);

        assert!(!already_wrapped.supported);

        assert_eq!(
            already_wrapped.reason,
            "command already uses claude-skills run"
        );
    }

    #[test]
    fn executable_command_prefixes_are_safe_for_their_target_shells() {
        let path = if cfg!(windows) {
            Path::new(r"C:\Users\Example User's Folder\.claude\claude-skills.exe")
        } else {
            Path::new("/home/example user's folder/.claude/claude-skills")
        };

        let bash_command = bash_command_for_executable_args(path, "run --");

        let platform_command = platform_default_command_for_executable_args(path, "run --");

        let hook_command = hook_command_for_executable_args(path, "hook session-start");

        if cfg!(windows) {
            assert_eq!(
                bash_command,
                r#"'C:\Users\Example User'\''s Folder\.claude\claude-skills.exe' run --"#
            );

            assert_eq!(
                platform_command,
                r#"& 'C:\Users\Example User''s Folder\.claude\claude-skills.exe' run --"#
            );

            assert!(hook_command
                .starts_with("powershell.exe -NoProfile -ExecutionPolicy Bypass -EncodedCommand "));

            assert!(!hook_command.contains("Example User"));
        } else {
            assert_eq!(
                bash_command,
                r#"'/home/example user'\''s folder/.claude/claude-skills' run --"#
            );

            assert_eq!(platform_command, bash_command);

            assert_eq!(
                hook_command,
                r#"'/home/example user'\''s folder/.claude/claude-skills' hook session-start"#
            );
        }
    }

    #[test]
    fn pre_tool_rerun_command_stays_bash_compatible_on_windows() {
        let rewrite = rewrite_command_text_for_shell("git status --short", RewriteShell::Bash);

        assert!(rewrite.supported);

        assert!(rewrite
            .rewritten_command
            .contains(" run -- git status --short"));

        assert!(
            !rewrite.rewritten_command.starts_with("& "),
            "Bash rerun command must not start with PowerShell call operator: {}",
            rewrite.rewritten_command
        );
    }

    #[test]
    fn manual_rewrite_uses_platform_default_shell() {
        let rewrite = rewrite_command_text("git status --short");

        assert!(rewrite.supported);

        if cfg!(windows) {
            assert!(
                rewrite.rewritten_command.starts_with("& '"),
                "Windows manual rewrite should be PowerShell-compatible: {}",
                rewrite.rewritten_command
            );
        } else {
            assert!(
                !rewrite.rewritten_command.starts_with("& "),
                "POSIX manual rewrite should not use PowerShell call operator: {}",
                rewrite.rewritten_command
            );
        }
    }
}
