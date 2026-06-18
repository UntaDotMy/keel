//! Purpose: Share compact-output helpers across command-specific adapters.
//! Caller: Built-in adapters for tests, git, search, files, build, lint, logs, and configured filters.
//! Dependencies: proxy adapter contracts, RunMeta, and TokenMeter.
//! Main Functions: make_result, compact_edges, signal_lines, command_program.
//! Side Effects: None; callers own persistence through the proxy.

use crate::proxy::adapter::CompactResult;
use crate::proxy::raw_store::RunMeta;
use crate::proxy::token_meter::TokenMeter;

pub const DEFAULT_EDGE_LINES: usize = 20;
pub const DEFAULT_LINE_LIMIT: usize = 80;

pub fn make_result(
    adapter_name: &str,
    summary: String,
    stdout: String,
    stderr: String,
    exit_code: i32,
    meta: &RunMeta,
    compacted: bool,
) -> CompactResult {
    let estimated_tokens_after = TokenMeter::estimate(&stdout) + TokenMeter::estimate(&stderr);
    let estimated_tokens_saved =
        meta.estimated_tokens_before as isize - estimated_tokens_after as isize;
    let savings_pct = if meta.estimated_tokens_before == 0 {
        0.0
    } else {
        (estimated_tokens_saved.max(0) as f64 / meta.estimated_tokens_before as f64) * 100.0
    };
    CompactResult {
        adapter_name: adapter_name.to_string(),
        compacted,
        summary,
        compact_stdout_bytes: stdout.len(),
        compact_stderr_bytes: stderr.len(),
        stdout,
        stderr,
        exit_code,
        raw_id: meta.raw_id.clone(),
        raw_path: meta.raw_path.clone(),
        original_stdout_bytes: meta.stdout_bytes,
        original_stderr_bytes: meta.stderr_bytes,
        estimated_tokens_before: meta.estimated_tokens_before,
        estimated_tokens_after,
        estimated_tokens_saved,
        savings_pct,
        warnings: Vec::new(),
    }
}

pub fn compact_edges(text: &str, label: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        return text.to_string();
    }
    let edge = DEFAULT_EDGE_LINES.min(max_lines / 2).max(5);
    let omitted = lines.len().saturating_sub(edge * 2);
    format!(
        "{label}: {} lines\n{}\n... omitted {omitted} lines; raw output saved for recovery ...\n{}",
        lines.len(),
        render_lines(&lines[..edge]),
        render_lines(&lines[lines.len() - edge..])
    )
}

pub fn signal_lines(text: &str, max_lines: usize) -> Vec<String> {
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
            selected.push(redact_possible_secret(trimmed));
        }
        if selected.len() >= max_lines {
            break;
        }
    }
    selected
}

pub fn redact_possible_secret(line: &str) -> String {
    let upper = line.to_ascii_uppercase();
    let named_secret = ["API_KEY=", "SECRET=", "TOKEN=", "PASSWORD=", "PRIVATE KEY"]
        .iter()
        .any(|needle| upper.contains(needle));
    let long_token = line
        .split(|character: char| {
            !character.is_ascii_alphanumeric() && character != '_' && character != '-'
        })
        .any(|part| {
            part.len() >= 40
                && part
                    .chars()
                    .filter(|character| character.is_ascii_alphanumeric())
                    .count()
                    >= 36
        });
    if named_secret || long_token {
        "[redacted possible secret; see raw output locally]".to_string()
    } else {
        line.to_string()
    }
}

fn render_lines(lines: &[&str]) -> String {
    lines
        .iter()
        .map(|line| redact_possible_secret(line))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn command_program(command: &str) -> String {
    command
        .split_whitespace()
        .next()
        .unwrap_or("command")
        .replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or("command")
        .trim_end_matches(".exe")
        .trim_end_matches(".cmd")
        .trim_end_matches(".bat")
        .to_ascii_lowercase()
}

pub fn normalized_command(program: &str, args: &[String]) -> String {
    if args.is_empty() {
        program.to_string()
    } else {
        format!("{} {}", program, args.join(" "))
    }
}

pub fn merge_streams(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    if stderr.trim().is_empty() {
        stdout.to_string()
    } else if stdout.trim().is_empty() {
        stderr.to_string()
    } else {
        format!("{stdout}\n{stderr}")
    }
}

/// Collapse consecutive identical lines into a single line with a repetition count.
/// Also deduplicates non-consecutive progress/status lines that repeat frequently.
/// e.g. "done\ndone\ndone" -> "done (3x)"
pub fn dedup_lines(text: &str) -> String {
    let mut result = String::new();
    let mut prev_original: Option<String> = None;
    let mut prev_trimmed: Option<String> = None;
    let mut count: usize = 0;
    for line in text.lines() {
        let trimmed = line.trim().to_string();
        if Some(trimmed.as_str()) == prev_trimmed.as_deref() {
            count += 1;
            continue;
        }
        if let Some(prev) = prev_original.take() {
            if count > 1 {
                append_dedup(&mut result, &prev, count);
            } else {
                result.push_str(&prev);
                result.push('\n');
            }
        }
        prev_original = Some(line.to_string());
        prev_trimmed = Some(trimmed);
        count = 1;
    }
    if let Some(prev) = prev_original.take() {
        if count > 1 {
            append_dedup(&mut result, &prev, count);
        } else {
            result.push_str(&prev);
            result.push('\n');
        }
    }
    result
}

fn append_dedup(result: &mut String, line: &str, count: usize) {
    result.push_str(line);
    result.push_str(&format!(" ({count}x)\n"));
}

/// Remove ANSI escape sequences (colors, cursor movement, progress bars).
/// Strips common patterns like \x1b[...m, \x1b[...G, \x1b[...K, and \r progress lines.
pub fn strip_ansi_escape(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\x1b' && chars.peek() == Some(&'[') {
            chars.next(); // consume '['
                          // consume until a CSI final byte (range @-~ per ECMA-48)
            while let Some(&next) = chars.peek() {
                if matches!(next, '\x20'..='\x7e') {
                    chars.next();
                    if matches!(next, '\x40'..='\x7e') {
                        break;
                    }
                } else {
                    break;
                }
            }
        } else if character == '\r' {
            // carriage return - skip if followed by non-newline (progress bar update)
            if chars.peek() != Some(&'\n') {
                continue;
            }
        } else {
            result.push(character);
        }
    }
    result
}

/// Compact JSON output to structure-only: keep object keys and array lengths, strip values.
/// e.g. {"name": "foo", "version": "1.0"} -> {"name": "<str>", "version": "<str>"}
pub fn compact_json_structure(text: &str) -> String {
    let trimmed = text.trim();
    if !trimmed.starts_with('{') && !trimmed.starts_with('[') {
        return text.to_string();
    }
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(value) => {
            let structure = describe_value(&value, 0);
            match serde_json::to_string_pretty(&structure) {
                Ok(rendered) => rendered,
                Err(_) => text.to_string(),
            }
        }
        Err(_) => text.to_string(),
    }
}

fn describe_value(value: &serde_json::Value, depth: usize) -> serde_json::Value {
    if depth > 8 {
        return serde_json::Value::String("...".to_string());
    }
    match value {
        serde_json::Value::Object(map) => {
            let mut description = serde_json::Map::new();
            for (key, val) in map.iter().take(32) {
                description.insert(key.clone(), describe_value(val, depth + 1));
            }
            serde_json::Value::Object(description)
        }
        serde_json::Value::Array(array_items) => {
            if array_items.is_empty() {
                serde_json::Value::Array(Vec::new())
            } else {
                let sample = describe_value(&array_items[0], depth + 1);
                let mut items = vec![sample];
                if array_items.len() > 1 {
                    items.push(serde_json::Value::String(format!(
                        "... {} more items",
                        array_items.len() - 1
                    )));
                }
                serde_json::Value::Array(items)
            }
        }
        serde_json::Value::String(string_value) => {
            if string_value.len() > 64 {
                serde_json::Value::String(format!("<str: {} chars>", string_value.len()))
            } else {
                serde_json::Value::String("<str>".to_string())
            }
        }
        serde_json::Value::Number(_) => serde_json::Value::String("<num>".to_string()),
        serde_json::Value::Bool(_) => serde_json::Value::String("<bool>".to_string()),
        serde_json::Value::Null => serde_json::Value::String("<null>".to_string()),
    }
}
