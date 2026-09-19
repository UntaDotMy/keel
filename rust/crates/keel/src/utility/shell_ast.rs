//! Purpose: Native shell AST, pipeline deconstruction, and obfuscation detection (K-Native-2).
//! Caller: `utility::decision`, `runner::shell_rewrite`, `runner::hook_lifecycle`.
//! Dependencies: std, serde, serde_json.
//! Main Functions: analyze_shell_ast, ShellAstAnalysis, ShellPipeline, ShellCommandStage.
//! Side Effects: None. Pure deterministic lexical parsing and AST safety analysis.

use serde::{Deserialize, Serialize};

/// Represents a single stage in a command pipeline or subshell.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShellCommandStage {
    pub raw_command: String,
    pub executable: String,
    pub arguments: Vec<String>,
    pub is_pipe_sink: bool,
    pub has_subshell: bool,
    pub unrolled_subshells: Vec<String>,
}

/// Analysis result of the shell AST.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShellAstAnalysis {
    pub raw_input: String,
    pub normalized_stream: String,
    pub stages: Vec<ShellCommandStage>,
    pub is_destructive: bool,
    pub is_obfuscated: bool,
    pub reasons: Vec<String>,
    pub risk_category: String,
}

/// Deconstruct a raw shell command line into structured AST stages and inspect for risk.
pub fn analyze_shell_ast(command: &str) -> ShellAstAnalysis {
    let mut stages = Vec::new();
    let mut reasons = Vec::new();
    let mut is_destructive = false;
    let mut is_obfuscated = false;

    // 1. Check for explicit base64 / hex obfuscation patterns
    if contains_base64_decode_pipe(command) {
        is_obfuscated = true;
        is_destructive = true;
        reasons.push("Obfuscated payload: Base64 decode piped into execution sink".to_string());
    }

    if contains_hex_escape_obfuscation(command) {
        is_obfuscated = true;
        reasons
            .push("Obfuscated payload: Hex-encoded executable or arguments detected".to_string());
    }

    // 2. Split into pipeline and sequential stages (respecting quotes)
    let raw_stages = split_pipeline_stages(command);

    for (idx, raw_stage) in raw_stages.iter().enumerate() {
        let is_last = idx == raw_stages.len().saturating_sub(1);
        let parsed_stage = parse_stage(raw_stage, !is_last);

        // Recursively inspect any unrolled subshells
        for subshell in &parsed_stage.unrolled_subshells {
            let sub_analysis = analyze_shell_ast(subshell);
            if sub_analysis.is_destructive {
                is_destructive = true;
                reasons.push(format!(
                    "Destructive command detected inside subshell '$({})': {}",
                    subshell,
                    sub_analysis.reasons.join("; ")
                ));
            }
            if sub_analysis.is_obfuscated {
                is_obfuscated = true;
                reasons.push(format!(
                    "Obfuscation detected inside subshell '$({})'",
                    subshell
                ));
            }
        }

        // Inspect executable and arguments of this stage
        let exe = parsed_stage.executable.to_ascii_lowercase();
        let args = &parsed_stage.arguments;

        // Pattern: Subshell in executable position e.g. $(echo rm -rf /)
        if exe.starts_with("$(") || exe.starts_with('`') {
            is_obfuscated = true;
            let inner_lower = exe.to_ascii_lowercase();
            if (inner_lower.contains("rm")
                && (inner_lower.contains("-r") || inner_lower.contains("-f")))
                || inner_lower.contains("delete")
                || inner_lower.contains("drop")
                || inner_lower.contains("format")
                || inner_lower.contains("wipe")
                || inner_lower.contains("dd")
            {
                is_destructive = true;
                reasons.push(format!(
                    "Dynamic command execution of destructive subshell: '{exe}'"
                ));
            } else {
                reasons.push(format!(
                    "Dynamic command execution in executable position: '{exe}'"
                ));
            }
        }

        // Pattern: rm -rf / or deletion verbs
        if exe == "rm" || exe.ends_with("/rm") || exe.ends_with("\\rm") {
            let has_recursive = args.iter().any(|a| {
                a == "-r"
                    || a == "-rf"
                    || a == "-fr"
                    || a == "-rfi"
                    || a.starts_with("-r")
                    || a.contains('r') && a.starts_with('-')
            });
            let has_force = args.iter().any(|a| a.contains('f') && a.starts_with('-'));
            let targets_critical = args.iter().any(|a| {
                a == "/"
                    || a == "/*"
                    || a == "~"
                    || a == "$HOME"
                    || a == "${HOME}"
                    || a == "C:\\"
                    || a == "C:/*"
                    || a == "."
                    || a == ".*"
            });

            if has_recursive && (has_force || targets_critical) {
                is_destructive = true;
                reasons.push(format!(
                    "Destructive rm invocation with recursive force: '{raw_stage}'"
                ));
            }
        }

        // Pattern: find ... -delete or find ... -exec rm
        if exe == "find" {
            if args.iter().any(|a| a == "-delete") {
                is_destructive = true;
                reasons.push("Destructive find command with '-delete' flag".to_string());
            }
            if args.iter().any(|a| a == "-exec") && args.iter().any(|a| a == "rm") {
                is_destructive = true;
                reasons.push("Destructive find command with '-exec rm' invocation".to_string());
            }
        }

        // Pattern: xargs rm
        if exe == "xargs" && args.iter().any(|a| a == "rm") {
            is_destructive = true;
            reasons.push("Bulk piped deletion via 'xargs rm'".to_string());
        }

        // Pattern: dd of=/dev/... or shred
        if exe == "dd" && args.iter().any(|a| a.starts_with("of=/dev/")) {
            is_destructive = true;
            reasons.push("Raw block device overwrite via dd".to_string());
        }
        if exe == "shred" || exe == "wipefs" {
            is_obfuscated = true;
            reasons.push(format!(
                "Potentially destructive operation via '{exe}' — escalation required"
            ));
        }

        // Pattern: mkfs / format
        if exe.starts_with("mkfs") || exe == "format" {
            is_destructive = true;
            reasons.push(format!("Filesystem format command detected: '{exe}'"));
        }

        // Pattern: remote pipe to shell
        if (exe == "curl" || exe == "wget") && !is_last {
            // Checked in conjunction with pipe sink
            is_destructive = true;
            reasons.push(
                "Remote code execution: download utility piped into downstream process".to_string(),
            );
        }

        // Pattern: git destructive resets
        if exe == "git" {
            if args.iter().any(|a| a == "reset") && args.iter().any(|a| a == "--hard") {
                is_destructive = true;
                reasons.push("Destructive git command: 'git reset --hard'".to_string());
            }
            if args.iter().any(|a| a == "clean")
                && (args.iter().any(|a| a == "-fd")
                    || args.iter().any(|a| a == "-f")
                    || args.iter().any(|a| a == "-xdf"))
            {
                is_destructive = true;
                reasons.push("Destructive untracked file purge: 'git clean -f'".to_string());
            }
            if args.iter().any(|a| a == "push")
                && (args.iter().any(|a| a == "--force") || args.iter().any(|a| a == "-f"))
            {
                is_destructive = true;
                reasons.push("Destructive remote git rewrite: 'git push --force'".to_string());
            }
        }

        stages.push(parsed_stage);
    }

    let category = if is_destructive {
        "Destructive"
    } else if is_obfuscated {
        "Risky"
    } else {
        "Safe"
    };

    ShellAstAnalysis {
        raw_input: command.to_string(),
        normalized_stream: normalize_collapsed_command(command),
        stages,
        is_destructive,
        is_obfuscated,
        reasons,
        risk_category: category.to_string(),
    }
}

/// Split input into pipeline/command stages respecting quotes and subshells.
pub fn split_pipeline_stages(command: &str) -> Vec<String> {
    let mut stages = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut subshell_depth = 0usize;
    let chars: Vec<char> = command.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let c = chars[i];

        if c == '\'' && !in_double && subshell_depth == 0 {
            in_single = !in_single;
            current.push(c);
            i += 1;
            continue;
        }

        if c == '"' && !in_single && subshell_depth == 0 {
            in_double = !in_double;
            current.push(c);
            i += 1;
            continue;
        }

        if !in_single {
            if c == '$' && i + 1 < len && chars[i + 1] == '(' {
                subshell_depth += 1;
                current.push(c);
                current.push('(');
                i += 2;
                continue;
            }
            if c == ')' && subshell_depth > 0 {
                subshell_depth -= 1;
                current.push(c);
                i += 1;
                continue;
            }
            if c == '`' {
                if subshell_depth > 0 {
                    subshell_depth -= 1;
                } else {
                    subshell_depth += 1;
                }
                current.push(c);
                i += 1;
                continue;
            }
        }

        // Split on |, ||, &&, ; if outside quotes and subshells
        if !in_single && !in_double && subshell_depth == 0 {
            if c == '|' {
                if i + 1 < len && chars[i + 1] == '|' {
                    // || operator
                    let trimmed = current.trim();
                    if !trimmed.is_empty() {
                        stages.push(trimmed.to_string());
                    }
                    current.clear();
                    i += 2;
                    continue;
                } else {
                    // Pipe operator
                    let trimmed = current.trim();
                    if !trimmed.is_empty() {
                        stages.push(trimmed.to_string());
                    }
                    current.clear();
                    i += 1;
                    continue;
                }
            } else if c == '&' && i + 1 < len && chars[i + 1] == '&' {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    stages.push(trimmed.to_string());
                }
                current.clear();
                i += 2;
                continue;
            } else if c == ';' {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    stages.push(trimmed.to_string());
                }
                current.clear();
                i += 1;
                continue;
            }
        }

        current.push(c);
        i += 1;
    }

    let trimmed = current.trim();
    if !trimmed.is_empty() {
        stages.push(trimmed.to_string());
    }

    if stages.is_empty() && !command.trim().is_empty() {
        stages.push(command.trim().to_string());
    }

    stages
}

/// Parse a single stage into executable, arguments, and extract nested subshells.
pub fn parse_stage(stage_raw: &str, is_pipe_sink: bool) -> ShellCommandStage {
    let mut tokens = Vec::new();
    let mut current_token = String::new();
    let mut unrolled_subshells = Vec::new();

    let mut in_single = false;
    let mut in_double = false;
    let mut in_subshell = false;
    let mut subshell_buf = String::new();

    let chars: Vec<char> = stage_raw.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let c = chars[i];

        if in_subshell {
            if c == ')' {
                in_subshell = false;
                unrolled_subshells.push(subshell_buf.clone());
                current_token.push_str(&format!("$({})", subshell_buf));
                subshell_buf.clear();
            } else {
                subshell_buf.push(c);
            }
            i += 1;
            continue;
        }

        if c == '$' && i + 1 < len && chars[i + 1] == '(' && !in_single {
            in_subshell = true;
            subshell_buf.clear();
            i += 2;
            continue;
        }

        if c == '\'' && !in_double {
            in_single = !in_single;
            // De-concatenate quotes by omitting literal quote marker in resolved token
            i += 1;
            continue;
        }

        if c == '"' && !in_single {
            in_double = !in_double;
            i += 1;
            continue;
        }

        if c.is_whitespace() && !in_single && !in_double {
            if !current_token.is_empty() {
                tokens.push(current_token.clone());
                current_token.clear();
            }
            i += 1;
            continue;
        }

        current_token.push(c);
        i += 1;
    }

    if !current_token.is_empty() {
        tokens.push(current_token);
    }

    let executable = tokens.first().cloned().unwrap_or_default();
    let arguments = if tokens.len() > 1 {
        tokens[1..].to_vec()
    } else {
        Vec::new()
    };

    ShellCommandStage {
        raw_command: stage_raw.to_string(),
        executable,
        arguments,
        is_pipe_sink,
        has_subshell: !unrolled_subshells.is_empty(),
        unrolled_subshells,
    }
}

/// Detect base64 decode pipelines.
fn contains_base64_decode_pipe(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    let has_b64_decoder = lower.contains("base64 -d")
        || lower.contains("base64 --decode")
        || lower.contains("frombase64string")
        || lower.contains("openssl enc -d -base64")
        || lower.contains("certutil -decode");

    let has_pipe = lower.contains('|');
    let has_execution_sink = lower.contains("| sh")
        || lower.contains("|sh")
        || lower.contains("| bash")
        || lower.contains("|bash")
        || lower.contains("| zsh")
        || lower.contains("|zsh")
        || lower.contains("| iex")
        || lower.contains("| powershell")
        || lower.contains("invoke-expression");

    has_b64_decoder && (has_pipe || has_execution_sink)
}

/// Detect hex escaped characters in command names like \x72\x6d ('rm').
fn contains_hex_escape_obfuscation(command: &str) -> bool {
    command.contains("\\x") || command.contains("$'\\x")
}

/// Collapses whitespace and removes quotation marks for baseline canonical representation.
fn normalize_collapsed_command(command: &str) -> String {
    let unquoted: String = command
        .chars()
        .filter(|&c| c != '\'' && c != '"' && c != '`')
        .collect();
    unquoted.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ast_detects_subshell_rm() {
        let cmd = "echo 'starting' && $(echo rm -rf /) && echo 'done'";
        let res = analyze_shell_ast(cmd);
        assert!(res.is_destructive);
        assert!(res.reasons.iter().any(|r| r.contains("subshell")));
    }

    #[test]
    fn test_ast_detects_deconcatenated_rm() {
        let cmd = "r''m -rf /";
        let res = analyze_shell_ast(cmd);
        assert!(res.is_destructive);
        assert_eq!(res.stages[0].executable, "rm");
    }

    #[test]
    fn test_ast_detects_base64_execution_sink() {
        let cmd = "echo cm0gLXJmIC8= | base64 -d | sh";
        let res = analyze_shell_ast(cmd);
        assert!(res.is_destructive);
        assert!(res.is_obfuscated);
    }

    #[test]
    fn test_ast_detects_find_delete() {
        let cmd = "find . -type f -name '*.tmp' -delete";
        let res = analyze_shell_ast(cmd);
        assert!(res.is_destructive);
        assert!(res.reasons.iter().any(|r| r.contains("-delete")));
    }

    #[test]
    fn test_ast_safe_command() {
        let cmd = "cargo test -p keel --lib -- utility::decision::tests";
        let res = analyze_shell_ast(cmd);
        assert!(!res.is_destructive);
        assert_eq!(res.risk_category, "Safe");
        assert_eq!(res.stages.len(), 1);
        assert_eq!(res.stages[0].executable, "cargo");
    }
}
