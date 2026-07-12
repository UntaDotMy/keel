//! Purpose: Render CompactResult into the concise agent-facing proxy output.
//! Caller: proxy::run after adapters finish compaction.
//! Dependencies: CompactResult and display_path formatting.
//! Main Functions: render_compact_result, render_ultra_compact_result.
//! Side Effects: None; caller writes the rendered text.

use crate::proxy::adapter::CompactResult;
use crate::runtime::display_path;

/// Max body lines kept by `--ultra` after the status line (plus raw footer).
const ULTRA_BODY_LINES: usize = 24;

pub fn render_compact_result(result: &CompactResult) -> String {
    let mut rendered = String::new();

    if result.adapter_name == "generic" || result.adapter_name == "errors-only" {
        rendered.push_str(&result.summary);
    } else if result.exit_code == 0 {
        rendered.push_str(&format!("PASS {}\n", result.summary));
    } else {
        rendered.push_str(&format!("FAIL {}\n", result.summary));
    }

    if !result.stdout.is_empty() {
        rendered.push('\n');
        rendered.push_str(&result.stdout);
        rendered.push('\n');
    }

    if !result.stderr.is_empty() {
        rendered.push('\n');
        rendered.push_str(&result.stderr);
        rendered.push('\n');
    }

    append_raw_footer(&mut rendered, result);
    rendered
}

/// Higher-aggression render for `keel run --ultra`: short status, body capped
/// to high-signal lines only, single-line raw pointer. Break-even still applies
/// in the caller.
pub fn render_ultra_compact_result(result: &CompactResult) -> String {
    let status = if result.exit_code == 0 { "ok" } else { "err" };
    let mut body_lines: Vec<String> = result
        .stdout
        .lines()
        .chain(result.stderr.lines())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    // Prefer lines that look like failures when truncating.
    if body_lines.len() > ULTRA_BODY_LINES {
        let errorish: Vec<String> = body_lines
            .iter()
            .filter(|line| {
                let lower = line.to_ascii_lowercase();
                [
                    "error", "fail", "panic", "fatal", "exception", "denied", "timeout",
                ]
                .iter()
                .any(|n| lower.contains(n))
            })
            .cloned()
            .collect();
        body_lines = if errorish.is_empty() {
            body_lines.into_iter().take(ULTRA_BODY_LINES).collect()
        } else {
            errorish.into_iter().take(ULTRA_BODY_LINES).collect()
        };
    }
    let mut rendered = format!(
        "{status} {} e={} n={}\n",
        result.adapter_name,
        result.exit_code,
        body_lines.len()
    );
    for line in &body_lines {
        rendered.push_str(line);
        rendered.push('\n');
    }
    rendered.push_str(&format!(
        "raw:{} saved:{}tok/{:.0}%",
        result.raw_id,
        result.estimated_tokens_saved.max(0),
        result.savings_pct
    ));
    rendered
}

fn append_raw_footer(rendered: &mut String, result: &CompactResult) {
    rendered.push_str(&format!(
        "\nraw: keel raw {}\nraw_path: {}\n",
        result.raw_id,
        display_path(&result.raw_path)
    ));
    rendered.push_str(&format!(
        "saved: {} tokens exact/o200k_base ({:.1}%)",
        result.estimated_tokens_saved.max(0),
        result.savings_pct
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_result() -> CompactResult {
        CompactResult {
            adapter_name: "generic".to_string(),
            compacted: true,
            summary: "summary".to_string(),
            compact_stdout_bytes: 0,
            compact_stderr_bytes: 0,
            stdout: (0..50)
                .map(|i| {
                    if i == 25 {
                        "ERROR boom".to_string()
                    } else {
                        format!("noise {i}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            stderr: String::new(),
            exit_code: 1,
            raw_id: "r1".to_string(),
            raw_path: PathBuf::from("/tmp/r1"),
            original_stdout_bytes: 1000,
            original_stderr_bytes: 0,
            estimated_tokens_before: 100,
            estimated_tokens_after: 20,
            estimated_tokens_saved: 80,
            savings_pct: 80.0,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn ultra_prefers_error_lines_and_stays_short() {
        let rendered = render_ultra_compact_result(&sample_result());
        assert!(rendered.starts_with("err "));
        assert!(rendered.contains("ERROR boom"));
        assert!(rendered.lines().count() < 30);
        assert!(rendered.contains("raw:r1"));
    }
}
