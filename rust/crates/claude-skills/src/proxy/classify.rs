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
            .map(|value| matches!(base_name(value).as_str(), "bash" | "sh" | "zsh"))
            .unwrap_or(false),
        has_shell_syntax(command_arguments),
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
        _ => words[index..].to_vec(),
    }
}

fn has_shell_syntax(words: &[String]) -> bool {
    words
        .iter()
        .any(|word| matches!(word.as_str(), "|" | "||" | "&&" | ";" | "<" | ">" | ">>"))
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
    use super::classify_command;
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
}
