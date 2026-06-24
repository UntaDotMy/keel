//! Purpose: Shell command rewriting, analysis, and compaction routing for keel run.
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

/// A destructive-command finding returned by [`detect_destructive_command`].
///
/// The PreToolUse hook turns this into a `permissionDecision: "deny"` payload so
/// the harness asks the user explicitly before the command runs, instead of
/// transparently rewriting it into `keel run --` and silently allowing it.
/// `pattern` is the human-readable rule that matched (for the deny reason);
/// `severity` lets the hook distinguish "always block" from "warn and ask".
pub struct DestructiveFinding {
    pub pattern: &'static str,
    pub severity: DestructiveSeverity,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DestructiveSeverity {
    /// Block by default — the command is almost certainly wrong as written.
    Block,
    /// Ask the user — the command may be intentional but carries real risk.
    Warn,
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

        // Additional test runners across ecosystems.
        "mocha" | "ava" | "cypress" | "karma" | "tap" | "gotestsum" | "bats" | "ctest" | "tox"
        | "nox" => "tests",

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

        "git" | "gh" | "jj" => "git",

        "rg" | "grep" => "search",

        "find" | "cat" | "sed" | "head" | "tail" | "ls" | "tree" | "jq" => "files",

        "ruff" | "eslint" | "mypy" | "biome" | "rubocop" | "flake8" | "golangci-lint" => "lint",

        // Additional linters/checkers across ecosystems.
        "pylint" | "pyright" | "shellcheck" | "hadolint" | "yamllint" | "stylelint" | "tflint"
        | "oxlint" | "standard" | "luacheck" | "vale" | "basedpyright" | "ty" | "markdownlint"
        | "markdownlint-cli2" => "lint",

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

        "brew" | "apt" | "apt-get" | "apk" | "choco" | "winget" | "scoop" => "build",

        // Containers + orchestration route to the containers adapter, NOT logs.
        // The prior `docker | kubectl | terraform | aws => logs` arm mislabeled
        // every one of these; only terraform actually stays on logs.
        "docker" | "kubectl" | "helm" | "podman" | "nerdctl" => "containers",

        // Cloud CLIs route to the dedicated cloud adapter (service-API JSON),
        // not logs. terraform plan/apply output is not service JSON, so it
        // deliberately stays on logs (handled below).
        "aws" | "az" | "gcloud" => "cloud",

        // Database clients route to the dedicated database adapter.
        "psql" | "mysql" | "mariadb" | "sqlite3" | "redis-cli" | "mongosh" | "mongo" => "database",

        // Bulk database exports stream file-like content -> logs adapter.
        "pg_dump" | "pg_dumpall" | "pg_restore" | "mysqldump" => "logs",

        // Infra-as-code (terraform) and HTTP probes stay on the logs adapter.
        "terraform" | "http" | "xh" | "httpie" => "logs",

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

        "make" | "cmake" | "ninja" | "msbuild" | "xcodebuild" => "build",

        "esbuild" | "swc" | "rollup" | "parcel" | "turbo" => "build",

        // Standalone build systems route to the build adapter.
        "bazel" | "buck" | "buck2" | "meson" | "scons" | "bear" => "build",

        // C/C++ compilers, task runners, and language/infra build tooling.
        "gcc" | "g++" | "clang" | "clang++" | "cc" | "c++" => "build",
        "just" | "task" | "mise" | "trunk" | "pre-commit" => "build",
        "swift" | "tofu" | "ansible" | "ansible-playbook" => "build",
        "liquibase" | "flyway" | "quarto" => "build",

        // JVM / other-language build+test multiplexers: a `test` task routes to
        // the tests adapter, everything else to build (mirrors the classifier).
        "sbt" | "lein" | "mill"
            if analysis
                .effective_fields
                .iter()
                .any(|arg| arg.contains("test")) =>
        {
            "tests"
        }

        "sbt" | "lein" | "mill" => "build",

        "cabal" | "stack" | "mix"
            if analysis
                .effective_fields
                .iter()
                .any(|arg| arg == "test" || arg.ends_with(".test")) =>
        {
            "tests"
        }

        "cabal" | "stack" | "mix" => "build",

        "curl" | "wget" => "logs",

        "pip" | "pip3" | "poetry" | "uv" => "build",

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

            reason: "command already uses keel run".into(),
        };
    }

    if !is_supported_noisy_command(&analysis.effective_fields) {
        return RewriteDecision {
            rewritten_command: String::new(),

            supported: false,

            reason: "no native keel compaction filter for command".into(),
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
    if fields.len() >= 2 && command_base_name(&fields[0]) == "keel" && fields[1] == "run" {
        return true;
    }

    fields.len() >= 2 && command_base_name(&fields[0]).starts_with("keel") && fields[1] == "run"
}

pub fn is_supported_noisy_command(fields: &[String]) -> bool {
    let Some(command) = fields.first().map(|value| command_base_name(value)) else {
        return false;
    };

    if matches!(
        command.as_str(),
        // Test runners.
        "cargo"
            | "npm"
            | "pnpm"
            | "yarn"
            | "bun"
            | "npx"
            | "pnpx"
            | "dlx"
            | "pytest"
            | "vitest"
            | "jest"
            | "playwright"
            | "mocha"
            | "ava"
            | "cypress"
            | "karma"
            | "tap"
            | "gotestsum"
            | "bats"
            | "ctest"
            | "tox"
            | "nox"
            | "deno"
            | "go"
            | "rspec"
            | "phpunit"
            // Build + bundlers (build adapter). cmake/ninja/msbuild/xcodebuild
            // and the JS bundlers were classified but never auto-wrapped, so the
            // build adapter went unused on their (often very noisy) output.
            | "gradle"
            | "mvn"
            | "make"
            | "cmake"
            | "ninja"
            | "msbuild"
            | "xcodebuild"
            | "bazel"
            | "buck"
            | "buck2"
            | "meson"
            | "scons"
            | "bear"
            | "gcc"
            | "g++"
            | "clang"
            | "clang++"
            | "cc"
            | "c++"
            | "just"
            | "task"
            | "mise"
            | "trunk"
            | "pre-commit"
            | "swift"
            | "tofu"
            | "ansible"
            | "ansible-playbook"
            | "liquibase"
            | "flyway"
            | "quarto"
            | "sbt"
            | "lein"
            | "mill"
            | "cabal"
            | "stack"
            | "mix"
            | "dotnet"
            | "tsc"
            | "prettier"
            | "esbuild"
            | "swc"
            | "rollup"
            | "parcel"
            | "turbo"
            | "webpack"
            | "vite"
            | "next"
            | "nx"
            | "rake"
            // Linters.
            | "ruff"
            | "eslint"
            | "mypy"
            | "biome"
            | "rubocop"
            | "flake8"
            | "golangci-lint"
            | "black"
            | "isort"
            | "pylint"
            | "pyright"
            | "shellcheck"
            | "hadolint"
            | "yamllint"
            | "stylelint"
            | "tflint"
            | "oxlint"
            | "standard"
            | "luacheck"
            | "vale"
            | "basedpyright"
            | "ty"
            | "markdownlint"
            | "markdownlint-cli2"
            // Containers + orchestration (containers adapter). helm/podman/nerdctl
            // had adapter coverage but no auto-wrap.
            | "docker"
            | "kubectl"
            | "helm"
            | "podman"
            | "nerdctl"
            // Cloud CLIs (cloud adapter). Only `aws` was wrapped; az/gcloud
            // classified to the cloud adapter but never reached it automatically.
            | "aws"
            | "az"
            | "gcloud"
            // Infra-as-code + HTTP probes (logs adapter).
            | "terraform"
            | "curl"
            | "wget"
            | "http"
            | "xh"
            | "httpie"
            // Database clients (database adapter). None were auto-wrapped before,
            // so the database adapter was dead on the automatic path. The Bash
            // tool is non-interactive, so these always carry a query flag here.
            | "psql"
            | "mysql"
            | "mariadb"
            | "sqlite3"
            | "redis-cli"
            | "mongosh"
            | "mongo"
            // Bulk database exports (logs adapter) — large, terminating output.
            | "pg_dump"
            | "pg_dumpall"
            | "pg_restore"
            | "mysqldump"
            // System log/service readers (logs adapter).
            | "journalctl"
            | "systemctl"
            // Version control + search + file readers.
            | "gh"
            | "git"
            | "git-lfs"
            | "jj"
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
            // Package managers (build adapter). apk/choco/winget/scoop were
            // classified but never auto-wrapped.
            | "pip"
            | "pip3"
            | "poetry"
            | "uv"
            | "bundle"
            | "composer"
            | "brew"
            | "apt"
            | "apt-get"
            | "apk"
            | "choco"
            | "winget"
            | "scoop"
    ) {
        return true;
    }

    command == "python"
        && fields.get(1).map(String::as_str) == Some("-m")
        && fields.get(2).map(String::as_str) == Some("pytest")
}

pub fn detect_destructive_command(fields: &[String]) -> Option<DestructiveFinding> {
    let Some(command) = fields.first().map(|value| command_base_name(value)) else {
        return None;
    };

    match command.as_str() {
        "rm" => {
            let has_recursive = fields.iter().any(|arg| {
                arg == "-r"
                    || arg == "-rf"
                    || arg == "-fr"
                    || arg == "-R"
                    || arg == "--recursive"
                    || (arg.starts_with('-')
                        && arg.len() > 1
                        && arg.contains('r')
                        && arg.contains('f'))
            });
            let has_force = fields.iter().any(|arg| {
                arg == "-f"
                    || arg == "--force"
                    || (arg.starts_with('-')
                        && arg.len() > 1
                        && !arg.starts_with("--")
                        && arg.contains('f'))
            });
            let targets_root = fields.iter().any(|arg| {
                arg == "/"
                    || arg == "/*"
                    || arg.starts_with("/home")
                    || arg.starts_with("/usr")
                    || arg == "~"
                    || arg.starts_with("~/")
            });
            let targets_many = fields.iter().filter(|arg| !arg.starts_with('-')).count() > 5;

            if has_recursive && has_force && targets_root {
                return Some(DestructiveFinding {
                    pattern: "rm -rf on system root",
                    severity: DestructiveSeverity::Block,
                });
            }
            if has_recursive && (targets_root || targets_many) {
                return Some(DestructiveFinding {
                    pattern: "rm -r on broad target",
                    severity: DestructiveSeverity::Warn,
                });
            }
        }
        "git" if fields.get(1).map(String::as_str) == Some("push") => {
            let has_force = fields
                .iter()
                .any(|arg| arg == "-f" || arg == "--force" || arg == "--force-with-lease");
            let targets_protected = fields
                .iter()
                .any(|arg| arg == "main" || arg == "master" || arg == "develop" || arg == "dev");

            if has_force && targets_protected {
                return Some(DestructiveFinding {
                    pattern: "git push --force to protected branch",
                    severity: DestructiveSeverity::Block,
                });
            }
            if has_force {
                return Some(DestructiveFinding {
                    pattern: "git push --force",
                    severity: DestructiveSeverity::Warn,
                });
            }
        }
        "chmod" | "chown" | "chgrp" => {
            let has_recursive = fields.iter().any(|arg| arg == "-R" || arg == "--recursive");
            let targets_root = fields.iter().any(|arg| {
                arg == "/" || arg.starts_with("/home") || arg.starts_with("/usr") || arg == "~"
            });
            let wide_permissions = fields
                .iter()
                .any(|arg| arg == "777" || arg == "a+rwx" || arg == "+x" || arg.contains("777"));

            if has_recursive && (targets_root || wide_permissions) {
                return Some(DestructiveFinding {
                    pattern: "recursive permission change on broad target",
                    severity: DestructiveSeverity::Warn,
                });
            }
        }
        "dd" => {
            let writes_device = fields.iter().any(|arg| {
                arg.starts_with("of=/dev/")
                    || arg.starts_with("of=/dev/sd")
                    || arg.starts_with("of=/dev/nvme")
            });
            if writes_device {
                return Some(DestructiveFinding {
                    pattern: "dd writing to block device",
                    severity: DestructiveSeverity::Block,
                });
            }
        }
        "fdisk" | "parted" | "gdisk" => {
            return Some(DestructiveFinding {
                pattern: "disk formatting/partitioning tool",
                severity: DestructiveSeverity::Block,
            });
        }
        _ if command.starts_with("mkfs") => {
            return Some(DestructiveFinding {
                pattern: "disk formatting/partitioning tool",
                severity: DestructiveSeverity::Block,
            });
        }
        _ => {}
    }

    let joined = fields.join(" ");
    let joined_lower = joined.to_ascii_lowercase();

    if joined_lower.contains("drop database") || joined_lower.contains("drop table") {
        return Some(DestructiveFinding {
            pattern: "DROP DATABASE or DROP TABLE statement",
            severity: DestructiveSeverity::Block,
        });
    }
    if joined_lower.contains("truncate table")
        || joined_lower.contains("delete from") && !joined_lower.contains("where")
    {
        return Some(DestructiveFinding {
            pattern: "TRUNCATE or unqualified DELETE",
            severity: DestructiveSeverity::Warn,
        });
    }

    None
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

        Err(_) => "keel run --".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_only_wraps_supported_noisy_commands() {
        let cargo = rewrite_command_text("cargo test --workspace");

        assert!(cargo.supported);

        assert!(cargo
            .rewritten_command
            .contains(" run -- cargo test --workspace"));

        let wrapped = rewrite_command_text("keel run -- cargo test --workspace");

        assert!(!wrapped.supported);

        assert_eq!(wrapped.reason, "command already uses keel run");

        let quiet = rewrite_command_text("echo hello");

        assert!(!quiet.supported);

        assert_eq!(quiet.reason, "no native keel compaction filter for command");
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
    fn auto_wrap_gate_covers_database_cloud_container_and_infra_commands() {
        // Regression: these all have dedicated adapters in the classifier but
        // were NOT in the auto-rewrite gate, so the adapter never fired on the
        // automatic PreToolUse path — the adapter was dead code in practice.
        for command in [
            "psql -c 'select 1'",
            "mysql -e 'show tables'",
            "sqlite3 db.sqlite '.tables'",
            "redis-cli ping",
            "mongosh --eval 'db.stats()'",
            "az vm list",
            "gcloud compute instances list",
            "helm install foo ./chart",
            "podman ps",
            "cmake --build .",
            "ninja -C build",
            "pg_dump mydb",
            "poetry install",
            "choco install foo",
        ] {
            let rewrite = rewrite_command_text(command);
            assert!(
                rewrite.supported,
                "auto-wrap gate must cover `{command}` (it has a dedicated adapter)"
            );
        }
    }

    #[test]
    fn rewrite_adapter_metadata_attributes_to_real_adapter() {
        // The metadata must name the adapter the classifier actually routes to,
        // not a catch-all. Prior bug: docker/kubectl/aws all reported "logs".
        assert_eq!(adapter_name_for_rewrite("docker ps"), "containers");
        assert_eq!(adapter_name_for_rewrite("kubectl get pods"), "containers");
        assert_eq!(adapter_name_for_rewrite("helm list"), "containers");
        assert_eq!(adapter_name_for_rewrite("aws s3 ls"), "cloud");
        assert_eq!(adapter_name_for_rewrite("az vm list"), "cloud");
        assert_eq!(adapter_name_for_rewrite("gcloud projects list"), "cloud");
        assert_eq!(adapter_name_for_rewrite("psql -c 'select 1'"), "database");
        assert_eq!(adapter_name_for_rewrite("redis-cli ping"), "database");
        // terraform stays on logs (plan/apply output is not service JSON).
        assert_eq!(adapter_name_for_rewrite("terraform plan"), "logs");
        // bulk exports stay on logs (file-like streaming output).
        assert_eq!(adapter_name_for_rewrite("pg_dump mydb"), "logs");
        assert_eq!(adapter_name_for_rewrite("cmake --build ."), "build");
    }

    /// Drift guard: every program the classifier routes to a *specific* adapter
    /// (anything other than the generic catch-all) MUST also be recognized by
    /// the auto-rewrite gate. Otherwise that adapter is unreachable on the
    /// automatic PreToolUse path. This is the structural fix for the
    /// database/cloud/container drift — the two surfaces can no longer diverge
    /// silently.
    #[test]
    fn every_specifically_classified_program_is_auto_wrappable() {
        use crate::proxy::classify::classify_command;
        use crate::proxy::command_ast::CommandKind;

        // One representative invocation per program that the classifier maps to
        // a non-generic CommandKind. If you add a program to the classifier with
        // a dedicated adapter, add it here too (and to is_supported_noisy_command).
        let samples: &[&[&str]] = &[
            &["cargo", "test"],
            &["pytest", "tests"],
            &["jest"],
            &["vitest"],
            &["playwright", "test"],
            &["mocha"],
            &["ava"],
            &["cypress", "run"],
            &["karma", "start"],
            &["tap"],
            &["gotestsum"],
            &["bats", "test"],
            &["ctest"],
            &["tox"],
            &["nox"],
            &["go", "test"],
            &["rspec"],
            &["phpunit"],
            &["gradle", "test"],
            &["mvn", "test"],
            &["dotnet", "test"],
            &["sbt", "test"],
            &["lein", "test"],
            &["mill", "__.test"],
            &["cabal", "test"],
            &["stack", "test"],
            &["mix", "test"],
            &["git", "status"],
            &["gh", "pr", "list"],
            &["jj", "status"],
            &["rg", "foo"],
            &["grep", "foo"],
            &["cat", "file"],
            &["head", "file"],
            &["tail", "file"],
            &["sed", "s/a/b/", "file"],
            &["jq", "."],
            &["ls", "-la"],
            &["find", "."],
            &["tree"],
            &["tsc", "--noEmit"],
            &["prettier", "--check", "."],
            &["esbuild", "in.js"],
            &["swc", "src"],
            &["rollup", "-c"],
            &["parcel", "build"],
            &["turbo", "run", "build"],
            &["make"],
            &["cmake", "--build", "."],
            &["ninja"],
            &["msbuild"],
            &["xcodebuild"],
            &["bazel", "build", "//..."],
            &["buck", "build"],
            &["buck2", "build"],
            &["meson", "compile"],
            &["scons"],
            &["bear", "--", "make"],
            &["gcc", "-c", "x.c"],
            &["g++", "-c", "x.cpp"],
            &["clang", "-c", "x.c"],
            &["cc", "-c", "x.c"],
            &["just", "build"],
            &["task", "build"],
            &["mise", "install"],
            &["trunk", "check"],
            &["pre-commit", "run"],
            &["swift", "build"],
            &["tofu", "plan"],
            &["ansible-playbook", "site.yml"],
            &["liquibase", "update"],
            &["flyway", "migrate"],
            &["quarto", "render"],
            &["webpack"],
            &["vite", "build"],
            &["next", "build"],
            &["eslint", "."],
            &["ruff", "check"],
            &["mypy", "."],
            &["biome", "check"],
            &["rubocop"],
            &["flake8"],
            &["golangci-lint", "run"],
            &["pylint", "pkg"],
            &["pyright"],
            &["shellcheck", "x.sh"],
            &["hadolint", "Dockerfile"],
            &["yamllint", "."],
            &["stylelint", "**/*.css"],
            &["tflint"],
            &["oxlint"],
            &["standard"],
            &["luacheck", "."],
            &["vale", "docs"],
            &["basedpyright"],
            &["ty", "check"],
            &["markdownlint", "."],
            &["markdownlint-cli2"],
            &["docker", "ps"],
            &["kubectl", "get", "pods"],
            &["helm", "list"],
            &["podman", "ps"],
            &["nerdctl", "ps"],
            &["aws", "s3", "ls"],
            &["az", "vm", "list"],
            &["gcloud", "projects", "list"],
            &["terraform", "plan"],
            &["curl", "https://x"],
            &["wget", "https://x"],
            &["psql", "-c", "select 1"],
            &["mysql", "-e", "show tables"],
            &["mariadb", "-e", "show tables"],
            &["sqlite3", "db", ".tables"],
            &["redis-cli", "ping"],
            &["mongosh", "--eval", "db.stats()"],
            &["pg_dump", "mydb"],
            &["mysqldump", "mydb"],
            &["journalctl", "-u", "x"],
            &["systemctl", "status", "x"],
            &["npm", "install"],
            &["pnpm", "install"],
            &["yarn", "add", "x"],
            &["bundle", "install"],
            &["composer", "install"],
            &["pip", "install", "x"],
            &["poetry", "install"],
            &["uv", "sync"],
            &["brew", "install", "x"],
            &["apt", "install", "x"],
            &["apk", "add", "x"],
            &["choco", "install", "x"],
            &["winget", "install", "x"],
            &["scoop", "install", "x"],
        ];

        for sample in samples {
            let fields: Vec<String> = sample.iter().map(|value| value.to_string()).collect();
            let ast = classify_command(&fields)
                .unwrap_or_else(|| panic!("classify failed for {sample:?}"));
            if ast.detected_kind == CommandKind::Unknown {
                // A bare program the classifier deliberately leaves Unknown
                // (e.g. `cargo` with no recognized subcommand) is allowed to
                // fall through; the gate need not wrap it.
                continue;
            }
            assert!(
                is_supported_noisy_command(&fields),
                "{sample:?} classifies as {:?} (a dedicated adapter) but the auto-rewrite gate does not wrap it — the adapter is unreachable on the automatic path",
                ast.detected_kind
            );
        }
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
            rewrite_command_text(r#""C:\Users\me\.claude\keel.exe" run -- cargo test"#);

        assert!(!already_wrapped.supported);

        assert_eq!(already_wrapped.reason, "command already uses keel run");
    }

    #[test]
    fn executable_command_prefixes_are_safe_for_their_target_shells() {
        let path = if cfg!(windows) {
            Path::new(r"C:\Users\Example User's Folder\.claude\keel.exe")
        } else {
            Path::new("/home/example user's folder/.claude/keel")
        };

        let bash_command = bash_command_for_executable_args(path, "run --");

        let platform_command = platform_default_command_for_executable_args(path, "run --");

        if cfg!(windows) {
            assert_eq!(
                bash_command,
                r#"'C:\Users\Example User'\''s Folder\.claude\keel.exe' run --"#
            );

            assert_eq!(
                platform_command,
                r#"& 'C:\Users\Example User''s Folder\.claude\keel.exe' run --"#
            );
        } else {
            assert_eq!(
                bash_command,
                r#"'/home/example user'\''s folder/.claude/keel' run --"#
            );

            assert_eq!(platform_command, bash_command);
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

    fn fields(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn destructive_rm_rf_on_root_is_blocked() {
        let finding = detect_destructive_command(&fields(&["rm", "-rf", "/"]));
        assert!(finding.is_some());
        assert_eq!(finding.unwrap().severity, DestructiveSeverity::Block);
    }

    #[test]
    fn destructive_rm_rf_on_home_is_blocked() {
        let finding = detect_destructive_command(&fields(&["rm", "-rf", "/home"]));
        assert!(finding.is_some());
        assert_eq!(finding.unwrap().severity, DestructiveSeverity::Block);
    }

    #[test]
    fn destructive_rm_recursive_on_broad_target_is_warned() {
        let finding =
            detect_destructive_command(&fields(&["rm", "-r", "a", "b", "c", "d", "e", "f"]));
        assert!(finding.is_some());
        assert_eq!(finding.unwrap().severity, DestructiveSeverity::Warn);
    }

    #[test]
    fn destructive_rm_single_file_is_not_flagged() {
        let finding = detect_destructive_command(&fields(&["rm", "file.txt"]));
        assert!(finding.is_none());
    }

    #[test]
    fn destructive_git_push_force_to_main_is_blocked() {
        let finding =
            detect_destructive_command(&fields(&["git", "push", "--force", "origin", "main"]));
        assert!(finding.is_some());
        assert_eq!(finding.unwrap().severity, DestructiveSeverity::Block);
    }

    #[test]
    fn destructive_git_push_force_to_feature_is_warned() {
        let finding =
            detect_destructive_command(&fields(&["git", "push", "-f", "origin", "feature/x"]));
        assert!(finding.is_some());
        assert_eq!(finding.unwrap().severity, DestructiveSeverity::Warn);
    }

    #[test]
    fn destructive_git_push_normal_is_not_flagged() {
        let finding = detect_destructive_command(&fields(&["git", "push", "origin", "main"]));
        assert!(finding.is_none());
    }

    #[test]
    fn destructive_chmod_recursive_777_is_warned() {
        let finding = detect_destructive_command(&fields(&["chmod", "-R", "777", "/var/www"]));
        assert!(finding.is_some());
        assert_eq!(finding.unwrap().severity, DestructiveSeverity::Warn);
    }

    #[test]
    fn destructive_dd_to_block_device_is_blocked() {
        let finding =
            detect_destructive_command(&fields(&["dd", "if=/dev/zero", "of=/dev/sda", "bs=1M"]));
        assert!(finding.is_some());
        assert_eq!(finding.unwrap().severity, DestructiveSeverity::Block);
    }

    #[test]
    fn destructive_mkfs_is_blocked() {
        let finding = detect_destructive_command(&fields(&["mkfs.ext4", "/dev/sda1"]));
        assert!(finding.is_some());
        assert_eq!(finding.unwrap().severity, DestructiveSeverity::Block);
    }

    #[test]
    fn destructive_sql_drop_database_is_blocked() {
        let finding =
            detect_destructive_command(&fields(&["psql", "-c", "DROP DATABASE production"]));
        assert!(finding.is_some());
        assert_eq!(finding.unwrap().severity, DestructiveSeverity::Block);
    }

    #[test]
    fn destructive_sql_drop_table_is_blocked() {
        let finding = detect_destructive_command(&fields(&["mysql", "-e", "DROP TABLE users"]));
        assert!(finding.is_some());
        assert_eq!(finding.unwrap().severity, DestructiveSeverity::Block);
    }

    #[test]
    fn destructive_sql_truncate_is_warned() {
        let finding = detect_destructive_command(&fields(&["psql", "-c", "TRUNCATE TABLE logs"]));
        assert!(finding.is_some());
        assert_eq!(finding.unwrap().severity, DestructiveSeverity::Warn);
    }

    #[test]
    fn destructive_sql_delete_without_where_is_warned() {
        let finding = detect_destructive_command(&fields(&["mysql", "-e", "DELETE FROM logs"]));
        assert!(finding.is_some());
        assert_eq!(finding.unwrap().severity, DestructiveSeverity::Warn);
    }

    #[test]
    fn destructive_sql_delete_with_where_is_not_flagged() {
        let finding =
            detect_destructive_command(&fields(&["psql", "-c", "DELETE FROM logs WHERE id = 1"]));
        assert!(finding.is_none());
    }

    #[test]
    fn destructive_safe_commands_are_not_flagged() {
        for safe in &[
            &["ls", "-la"][..],
            &["git", "status"],
            &["cargo", "test"],
            &["npm", "install"],
            &["docker", "ps"],
            &["kubectl", "get", "pods"],
        ] {
            let finding = detect_destructive_command(&fields(safe));
            assert!(
                finding.is_none(),
                "safe command {safe:?} should not be flagged"
            );
        }
    }

    #[test]
    fn destructive_empty_command_is_not_flagged() {
        let finding = detect_destructive_command(&fields(&[]));
        assert!(finding.is_none());
    }
}
