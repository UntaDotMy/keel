pub fn is_denied_command(cmd: &str) -> bool {
    let low = cmd.to_ascii_lowercase();
    low.contains("git commit")
        || low.contains("git push")
        || low.contains("git rebase")
        || low.contains("git branch")
}

pub fn clip_output(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let head = 1500usize;
    let tail = 2000usize;
    let h = &text[..head.min(text.len())];
    let t_start = text.len().saturating_sub(tail);
    format!(
        "{h}\n... clipped {} chars ...\n{}",
        text.len() - head - tail,
        &text[t_start..]
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
}
