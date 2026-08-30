pub fn is_denied_command(cmd: &str) -> bool {
    let tokens: Vec<String> = cmd
        .split_whitespace()
        .map(|token| token.trim_matches(['\'', '"']).to_ascii_lowercase())
        .collect();
    tokens.iter().enumerate().any(|(index, token)| {
        (token == "git" || token.ends_with("/git") || token.ends_with("\\git.exe"))
            && tokens[index + 1..].iter().any(|argument| {
                matches!(
                    argument.as_str(),
                    "commit" | "push" | "rebase" | "branch" | "clean"
                ) || (argument == "reset"
                    && tokens[index + 1..].iter().any(|value| value == "--hard"))
            })
    })
}

pub fn is_denied_argv(program: &str, arguments: &[String]) -> bool {
    let executable = std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    if matches!(executable.as_str(), "git" | "git.exe") {
        let lowered: Vec<String> = arguments
            .iter()
            .map(|argument| argument.to_ascii_lowercase())
            .collect();
        return lowered.iter().any(|argument| {
            matches!(
                argument.as_str(),
                "commit" | "push" | "rebase" | "branch" | "clean"
            ) || (argument == "reset" && lowered.iter().any(|value| value == "--hard"))
        });
    }
    let command_line = std::iter::once(program)
        .chain(arguments.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    is_denied_command(&command_line)
}

pub fn clip_output(text: &str, max_chars: usize) -> String {
    let total_chars = text.chars().count();
    if total_chars <= max_chars {
        return text.to_string();
    }
    if max_chars < 32 {
        return text.chars().take(max_chars).collect();
    }
    let marker_reserve = 31usize.min(max_chars);
    let available = max_chars.saturating_sub(marker_reserve);
    let head_chars = available.saturating_mul(3) / 7;
    let tail_chars = available.saturating_sub(head_chars);
    let head_end = text
        .char_indices()
        .nth(head_chars)
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    let tail_start = if tail_chars == 0 {
        text.len()
    } else {
        text.char_indices()
            .rev()
            .nth(tail_chars - 1)
            .map(|(index, _)| index)
            .unwrap_or(text.len())
    };
    let clipped_chars = total_chars.saturating_sub(head_chars + tail_chars);
    format!(
        "{}\n... clipped {clipped_chars} chars ...\n{}",
        &text[..head_end],
        &text[tail_start..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn denylist_blocks_git_commit() {
        assert!(is_denied_command("git commit -m foo"));
        assert!(!is_denied_command("pytest -q"));
    }
    #[test]
    fn huge_output_clipped() {
        let big = "x".repeat(10000);
        let clipped = clip_output(&big, 4000);
        assert!(clipped.len() < big.len());
        assert!(clipped.contains("clipped"));
    }

    #[test]
    fn unicode_output_is_clipped_on_character_boundaries() {
        let big = "🦀".repeat(5000);
        let clipped = clip_output(&big, 4000);
        assert!(clipped.contains("clipped"));
        assert!(clipped.is_char_boundary(clipped.len()));
    }

    #[test]
    fn tiny_output_budget_never_panics_or_exceeds_budget() {
        let clipped = clip_output("🦀abcdef", 3);
        assert_eq!(clipped.chars().count(), 3);
    }

    #[test]
    fn denylist_blocks_git_global_options_before_subcommand() {
        assert!(is_denied_argv(
            "git",
            &[
                "-C".into(),
                "candidate".into(),
                "commit".into(),
                "-am".into(),
                "x".into()
            ]
        ));
    }
}
