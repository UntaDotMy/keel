//! Purpose: Provide the proxy-facing command classification entry point.
//! Caller: proxy::run before adapter selection and adapter rewrite.
//! Dependencies: CommandAst and the process working directory.
//! Main Functions: classify_command.
//! Side Effects: Reads current working directory when classification is requested.

use crate::proxy::command_ast::CommandAst;
use crate::runner::shell_rewrite::{command_base_name, is_env_assignment};

pub fn classify_command(command_arguments: &[String]) -> Option<CommandAst> {
    let original_command = command_arguments.join(" ");
    let words = effective_command_fields(command_arguments, 0);
    let program = words.first()?.clone();
    let args = words.iter().skip(1).cloned().collect();
    let cwd = std::env::current_dir().unwrap_or_default();
    Some(CommandAst::from_parts(
        original_command,
        program,
        args,
        cwd,
        command_arguments
            .first()
            .map(|value| {
                matches!(
                    base_name(value).as_str(),
                    // why: `pwsh`/`powershell`/`cmd` already wrap a command line
                    // (e.g. the MCP run_command tool emits `pwsh -NoProfile
                    // -Command "…"`). Failing to mark them shell_wrapped made the
                    // proxy see the inner `|`/`>` and wrap AGAIN through the
                    // platform shell — the double-wrap that mangled quoting.
                    "bash" | "sh" | "zsh" | "pwsh" | "powershell" | "cmd"
                )
            })
            .unwrap_or(false),
        contains_shell_syntax(command_arguments),
    ))
}

fn effective_command_fields(words: &[String], depth: usize) -> Vec<String> {
    if depth > 4 {
        return words.to_vec();
    }
    let mut index = 0usize;
    while words
        .get(index)
        .map(|value| is_env_assignment(value))
        .unwrap_or(false)
    {
        index += 1;
    }
    let Some(command) = words.get(index).map(|value| base_name(value)) else {
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
            if index >= words.len() {
                words[..1].to_vec()
            } else {
                effective_command_fields(&words[index..], depth + 1)
            }
        }
        "time" | "command" | "exec" | "nohup" => {
            if index + 1 >= words.len() {
                words[index..].to_vec()
            } else {
                effective_command_fields(&words[index + 1..], depth + 1)
            }
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
            if index >= words.len() {
                words[..1].to_vec()
            } else {
                effective_command_fields(&words[index..], depth + 1)
            }
        }
        "bash" | "sh" | "zsh" => {
            for (offset, word) in words[index + 1..].iter().enumerate() {
                if word.starts_with('-') && word.contains('c') {
                    if let Some(shell_command) = words.get(index + offset + 2) {
                        let nested = split_shell_words(shell_command);
                        return effective_command_fields(&nested, depth + 1);
                    }
                }
            }
            words[index..].to_vec()
        }
        // why: Windows shells carry the command line after `-Command`/`-c`
        // (pwsh / powershell) or `/C` (cmd). On Windows the inner command line
        // arrives as a *single* argv token (the quoted `-Command "…"` string),
        // so split it into words like the `bash -c` branch does — recursing on
        // the raw token would treat the whole line as one program name.
        "pwsh" | "powershell" | "cmd" => {
            for (offset, word) in words[index + 1..].iter().enumerate() {
                let lowered = word.to_ascii_lowercase();
                if lowered == "-command" || lowered == "-c" || lowered == "/c" {
                    if let Some(shell_command) = words.get(index + 1 + offset + 1) {
                        let nested = split_shell_words(shell_command);
                        return effective_command_fields(&nested, depth + 1);
                    }
                }
            }
            words[index..].to_vec()
        }
        _ => words[index..].to_vec(),
    }
}

/// Detect shell syntax for both proxy wrapping and passthrough.
/// Both paths must share this function to avoid unsafe direct execution.
///
/// Operators match whole tokens: a pipe inside one argv word (e.g. a quoted
/// `rg "error|warning"`) is data, not syntax. Numbered/dup redirects (`2>`,
/// `&>`, `2>&1`, glued or bare) and substitution/grouping characters
/// (`$`, backtick, parens) are detected anywhere in a token.
pub(crate) fn contains_shell_syntax(words: &[String]) -> bool {
    words.iter().any(|word| {
        matches!(
            word.as_str(),
            "|" | "||" | "&&" | "&" | ";" | "<" | ">" | ">>"
        ) || looks_like_redirect(word)
            || word
                .chars()
                .any(|character| matches!(character, '$' | '`' | '(' | ')'))
    })
}

fn looks_like_redirect(word: &str) -> bool {
    let stripped = word.trim_start_matches(|character: char| character.is_ascii_digit());
    let after_dup = stripped.strip_prefix('&').unwrap_or(stripped);
    after_dup.starts_with('>')
}

/// Case-insensitive base-name used by classification matchers (`bash`, `sudo`,
/// `env`, ...) so that `BASH.EXE` and `bash` collapse to the same key. Defers
/// to the canonical `command_base_name` for the path stripping and
/// extension-trim logic, then lower-cases the result.
fn base_name(command: &str) -> String {
    command_base_name(command).to_ascii_lowercase()
}

fn split_shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for character in command.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(quote_character) = quote {
            if character == quote_character {
                quote = None;
            } else {
                current.push(character);
            }
            continue;
        }
        if character == '\'' || character == '"' {
            quote = Some(character);
            continue;
        }
        if character.is_whitespace() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }
        if matches!(character, '|' | '&' | ';' | '<' | '>') {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            words.push(character.to_string());
            continue;
        }
        current.push(character);
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::{classify_command, contains_shell_syntax};
    use crate::proxy::command_ast::CommandKind;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn classifies_wrapped_test_commands() {
        let ast =
            classify_command(&args(&["env", "RUST_BACKTRACE=1", "cargo", "test"])).expect("ast");
        assert_eq!(ast.program, "cargo");
        assert_eq!(ast.detected_kind, CommandKind::Test);

        let ast = classify_command(&args(&["bash", "-lc", "pytest tests -q"])).expect("ast");
        assert_eq!(ast.program, "pytest");
        assert_eq!(ast.detected_kind, CommandKind::Test);
        assert!(ast.shell_wrapped);

        let ast = classify_command(&args(&["time", "go", "test", "./..."])).expect("ast");
        assert_eq!(ast.program, "go");
        assert_eq!(ast.detected_kind, CommandKind::Test);
    }

    #[test]
    fn classifies_git_search_and_shell_syntax() {
        let ast = classify_command(&args(&["git", "diff", "--cached"])).expect("ast");
        assert_eq!(ast.detected_kind, CommandKind::Git);

        let ast = classify_command(&args(&["rg", "foo", ".", "|", "head"])).expect("ast");
        assert_eq!(ast.detected_kind, CommandKind::Search);
        assert!(ast.has_shell_syntax);

        let ast = classify_command(&args(&["rg", "error|warning", "src"])).expect("ast");
        assert_eq!(ast.detected_kind, CommandKind::Search);
        assert!(!ast.has_shell_syntax);
    }

    #[test]
    fn windows_shell_wrappers_are_marked_wrapped_and_unwrapped() {
        // Regression: the MCP run_command tool emits `pwsh -NoProfile -Command
        // "<cmd>"`. classify_command only recognized bash/sh/zsh as wrappers, so
        // the inner `|` made has_shell_syntax fire and the proxy wrapped the
        // whole thing AGAIN — the double-wrap that mangled quoting. A Windows
        // shell must be flagged shell_wrapped AND unwrapped to its inner program.
        let ast = classify_command(&args(&[
            "pwsh",
            "-NoProfile",
            "-Command",
            "Get-Content log.txt | Select-Object -First 2",
        ]))
        .expect("ast");
        assert!(
            ast.shell_wrapped,
            "pwsh -Command must be recognized as already shell-wrapped"
        );
        assert_eq!(
            ast.program, "Get-Content",
            "classification should unwrap to the inner cmdlet"
        );

        // cmd /C carries the command line as a single token (how
        // platform_shell_command_parts emits it).
        let ast = classify_command(&args(&["cmd", "/C", "dir /b"])).expect("ast");
        assert!(ast.shell_wrapped, "cmd /C must be marked shell-wrapped");
        assert_eq!(ast.program, "dir");

        // powershell.exe (5.1) uses the same -Command surface; the inner line is
        // one quoted token.
        let ast = classify_command(&args(&["powershell", "-Command", "cargo test"])).expect("ast");
        assert!(ast.shell_wrapped);
        assert_eq!(ast.program, "cargo");
        assert_eq!(ast.detected_kind, CommandKind::Test);
    }

    #[test]
    fn redirects_and_substitution_are_shell_syntax() {
        // Capture and passthrough paths must agree on shell syntax.
        for argv in [
            vec!["prog", "arg", "2>", "err.log"],
            vec!["prog", "arg", "2>err.log"],
            vec!["prog", "arg", "&>all.log"],
            vec!["prog", "arg", "2>&1"],
            vec!["prog", "`sub`"],
            vec!["echo", "$(date)"],
            vec!["prog", "subshell", "(a; b)"],
        ] {
            let ast = classify_command(&args(&argv)).expect("ast");
            assert!(ast.has_shell_syntax, "expected shell syntax: {argv:?}");
            assert!(
                contains_shell_syntax(&args(&argv)),
                "passthrough detector must agree: {argv:?}"
            );
        }
        // Pipes inside one quoted word are data, not shell syntax.
        // An unquoted `>` remains a redirect and is tested above.
        let argv = vec!["rg", "error|warning", "src"];
        let ast = classify_command(&args(&argv)).expect("ast");
        assert!(!ast.has_shell_syntax, "expected shell-free: {argv:?}");
    }
}
