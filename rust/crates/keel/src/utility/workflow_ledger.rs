//! Purpose: Per-workspace state ledger backing the `keel workflow start|cockpit|finish` surface.
//! Caller: utility::memory::run_workflow_command for the start, cockpit, and finish subcommands.
//! Dependencies: std::fs, std::path, std::time, crate::json::{write_indented, Value}, crate::runtime::display_path.
//! Main Functions: ledger_directory, entry_path, write_entry, read_entry, list_entries,
//!   create_entry, close_entry, entry_to_value, current_timestamp_millis,
//!   format_timestamp_iso8601, next_entry_id, allocate_unique_entry_id, parse_entry_text.
//! Side Effects: Reads and writes JSON files under `<claude-home>/workflow/`. No global state.
//!
//! Storage shape: one JSON file per entry at `<claude-home>/workflow/<id>.json`.
//! Files contain a flat object of seven string fields. Cockpit renders all open entries
//! plus a tail of recently closed entries.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::json::{write_indented, Value};
use crate::runtime::{display_path, safe_path_segment, write_text};

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: String,
    pub request: String,
    pub preset: String,
    pub status: String,
    pub started_at: String,
    pub finished_at: String,
    pub proof: String,
}

pub const STATUS_OPEN: &str = "open";
pub const STATUS_CLOSED: &str = "closed";

pub fn ledger_directory(claude_home: &Path) -> PathBuf {
    claude_home.join("workflow")
}

pub fn entry_path(claude_home: &Path, id: &str) -> PathBuf {
    ledger_directory(claude_home).join(format!("{id}.json"))
}

/// Validate `id` as a single safe path segment before joining it into a ledger
/// entry path, so a CLI `--id` like `../../foo` or an absolute path cannot steer
/// the `{id}.json` join outside the workflow directory. Guarding inside
/// write_entry/read_entry covers every caller by construction.
fn validated_entry_path(claude_home: &Path, id: &str) -> Result<PathBuf, String> {
    match safe_path_segment(id) {
        Some(segment) => Ok(entry_path(claude_home, &segment)),
        None => Err(format!(
            "invalid workflow id {id:?}: must be a single safe path segment"
        )),
    }
}

pub fn current_timestamp_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

pub fn next_entry_id(millis_since_epoch: u128) -> String {
    format!("wf-{millis_since_epoch:x}")
}

pub fn allocate_unique_entry_id(claude_home: &Path, base_millis: u128) -> Result<String, String> {
    let base = next_entry_id(base_millis);
    if !entry_path(claude_home, &base).exists() {
        return Ok(base);
    }
    for counter in 1..1000u32 {
        let candidate = format!("{base}-{counter}");
        if !entry_path(claude_home, &candidate).exists() {
            return Ok(candidate);
        }
    }
    Err("unable to allocate unique workflow id".into())
}

pub fn create_entry(id: String, request: String, preset: String, started_at: String) -> Entry {
    Entry {
        id,
        request,
        preset,
        status: STATUS_OPEN.into(),
        started_at,
        finished_at: String::new(),
        proof: String::new(),
    }
}

pub fn close_entry(entry: Entry, finished_at: String, proof: String) -> Entry {
    Entry {
        status: STATUS_CLOSED.into(),
        finished_at,
        proof,
        ..entry
    }
}

pub fn entry_to_value(entry: &Entry) -> Value {
    Value::Object(vec![
        ("id".into(), Value::String(entry.id.clone())),
        ("request".into(), Value::String(entry.request.clone())),
        ("preset".into(), Value::String(entry.preset.clone())),
        ("status".into(), Value::String(entry.status.clone())),
        ("startedAt".into(), Value::String(entry.started_at.clone())),
        (
            "finishedAt".into(),
            Value::String(entry.finished_at.clone()),
        ),
        ("proof".into(), Value::String(entry.proof.clone())),
    ])
}

pub fn write_entry(claude_home: &Path, entry: &Entry) -> Result<PathBuf, String> {
    let path = validated_entry_path(claude_home, &entry.id)?;
    let directory = ledger_directory(claude_home);
    fs::create_dir_all(&directory)
        .map_err(|error| format!("create {}: {error}", display_path(&directory)))?;
    let mut serialized = Vec::<u8>::new();
    write_indented(&mut serialized, &entry_to_value(entry))
        .map_err(|error| format!("serialize entry {}: {error}", entry.id))?;
    let text = String::from_utf8(serialized)
        .map_err(|error| format!("serialize entry {}: non-utf8 output: {error}", entry.id))?;
    // Atomic temp+fsync+rename so a crash never leaves a truncated entry that
    // would then poison list_entries / cockpit.
    write_text(&path, &text)?;
    Ok(path)
}

pub fn read_entry(claude_home: &Path, id: &str) -> Result<Option<Entry>, String> {
    let path = validated_entry_path(claude_home, id)?;
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read {}: {error}", display_path(&path))),
    };
    let entry = parse_entry_text(&text)
        .map_err(|error| format!("parse {}: {error}", display_path(&path)))?;
    Ok(Some(entry))
}

pub fn list_entries(claude_home: &Path) -> Result<Vec<Entry>, String> {
    let directory = ledger_directory(claude_home);
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let read_iter = fs::read_dir(&directory)
        .map_err(|error| format!("read {}: {error}", display_path(&directory)))?;
    let mut entries = Vec::new();
    for read_result in read_iter {
        let dir_entry =
            read_result.map_err(|error| format!("read {}: {error}", display_path(&directory)))?;
        let path = dir_entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        // Skip an unreadable/unparseable entry rather than aborting the listing.
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("skip {}: {error}", display_path(&path));
                continue;
            }
        };
        match parse_entry_text(&text) {
            Ok(entry) => entries.push(entry),
            Err(error) => {
                eprintln!("skip {}: {error}", display_path(&path));
                continue;
            }
        }
    }
    entries.sort_by(|left, right| {
        left.started_at
            .cmp(&right.started_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(entries)
}

pub fn parse_entry_text(text: &str) -> Result<Entry, String> {
    let fields = parse_object_of_strings(text)?;
    let mut entry = Entry {
        id: String::new(),
        request: String::new(),
        preset: String::new(),
        status: String::new(),
        started_at: String::new(),
        finished_at: String::new(),
        proof: String::new(),
    };
    for (key, value) in fields {
        match key.as_str() {
            "id" => entry.id = value,
            "request" => entry.request = value,
            "preset" => entry.preset = value,
            "status" => entry.status = value,
            "startedAt" => entry.started_at = value,
            "finishedAt" => entry.finished_at = value,
            "proof" => entry.proof = value,
            _ => {}
        }
    }
    if entry.id.is_empty() {
        return Err("entry missing id field".into());
    }
    Ok(entry)
}

pub fn format_timestamp_iso8601(millis_since_epoch: u128) -> String {
    let total_seconds = (millis_since_epoch / 1000) as i64;
    let (year, month, day, hour, minute, second) = unix_seconds_to_civil(total_seconds);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn unix_seconds_to_civil(unix_seconds: i64) -> (i32, u32, u32, u32, u32, u32) {
    let seconds_per_day: i64 = 86400;
    let days_since_epoch = unix_seconds.div_euclid(seconds_per_day);
    let time_of_day = unix_seconds.rem_euclid(seconds_per_day);
    let z = days_since_epoch + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let day_of_era = z - era * 146097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let civil_year_basis = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_phase = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_phase + 2) / 5 + 1) as u32;
    let month = if month_phase < 10 {
        (month_phase + 3) as u32
    } else {
        (month_phase - 9) as u32
    };
    let year = (civil_year_basis + if month <= 2 { 1 } else { 0 }) as i32;
    let hour = (time_of_day / 3600) as u32;
    let minute = ((time_of_day % 3600) / 60) as u32;
    let second = (time_of_day % 60) as u32;
    (year, month, day, hour, minute, second)
}

pub(crate) fn parse_object_of_strings(text: &str) -> Result<Vec<(String, String)>, String> {
    let bytes = text.as_bytes();
    let mut index = 0;
    skip_whitespace(bytes, &mut index);
    if index >= bytes.len() || bytes[index] != b'{' {
        return Err("expected '{'".into());
    }
    index += 1;
    let mut fields = Vec::new();
    loop {
        skip_whitespace(bytes, &mut index);
        if index >= bytes.len() {
            return Err("unterminated object".into());
        }
        if bytes[index] == b'}' {
            return Ok(fields);
        }
        let key = parse_string_literal(bytes, &mut index)?;
        skip_whitespace(bytes, &mut index);
        if index >= bytes.len() || bytes[index] != b':' {
            return Err(format!("expected ':' after key {key:?}"));
        }
        index += 1;
        skip_whitespace(bytes, &mut index);
        let value = parse_string_literal(bytes, &mut index)?;
        fields.push((key, value));
        skip_whitespace(bytes, &mut index);
        if index >= bytes.len() {
            return Err("unterminated object".into());
        }
        match bytes[index] {
            b',' => {
                index += 1;
                continue;
            }
            b'}' => return Ok(fields),
            other => return Err(format!("expected ',' or '}}', got {:?}", other as char)),
        }
    }
}

fn parse_string_literal(bytes: &[u8], index: &mut usize) -> Result<String, String> {
    if *index >= bytes.len() || bytes[*index] != b'"' {
        return Err("expected string literal".into());
    }
    *index += 1;
    let mut output = String::new();
    while *index < bytes.len() {
        let byte = bytes[*index];
        if byte == b'"' {
            *index += 1;
            return Ok(output);
        }
        if byte == b'\\' {
            *index += 1;
            if *index >= bytes.len() {
                return Err("trailing backslash in string literal".into());
            }
            match bytes[*index] {
                b'"' => output.push('"'),
                b'\\' => output.push('\\'),
                b'/' => output.push('/'),
                b'n' => output.push('\n'),
                b'r' => output.push('\r'),
                b't' => output.push('\t'),
                b'b' => output.push('\x08'),
                b'f' => output.push('\x0c'),
                b'u' => {
                    if *index + 4 >= bytes.len() {
                        return Err("incomplete \\u escape".into());
                    }
                    let hex_text = std::str::from_utf8(&bytes[*index + 1..*index + 5])
                        .map_err(|_| "non-utf8 in \\u escape".to_string())?;
                    let code_point = u32::from_str_radix(hex_text, 16)
                        .map_err(|_| format!("invalid \\u hex: {hex_text}"))?;
                    if let Some(character) = char::from_u32(code_point) {
                        output.push(character);
                    }
                    *index += 4;
                }
                other => return Err(format!("invalid escape \\{}", other as char)),
            }
            *index += 1;
        } else {
            let remainder = std::str::from_utf8(&bytes[*index..])
                .map_err(|_| "invalid utf8 in string literal".to_string())?;
            let character = remainder
                .chars()
                .next()
                .ok_or_else(|| "empty string literal body".to_string())?;
            output.push(character);
            *index += character.len_utf8();
        }
    }
    Err("unterminated string literal".into())
}

fn skip_whitespace(bytes: &[u8], index: &mut usize) {
    while *index < bytes.len() {
        match bytes[*index] {
            b' ' | b'\t' | b'\n' | b'\r' => *index += 1,
            _ => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static UNIQUE_TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_directory(label: &str) -> PathBuf {
        let counter = UNIQUE_TEST_DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let directory = std::env::temp_dir().join(format!("keel-workflow-{label}-{pid}-{counter}"));
        if directory.exists() {
            let _ = fs::remove_dir_all(&directory);
        }
        fs::create_dir_all(&directory).expect("create temp directory");
        directory
    }

    #[test]
    fn entry_path_uses_json_extension_under_workflow_directory() {
        let claude_home = PathBuf::from("/fake/home");
        let path = entry_path(&claude_home, "wf-abc");
        assert_eq!(path, PathBuf::from("/fake/home/workflow/wf-abc.json"));
    }

    #[test]
    fn create_entry_starts_open_with_empty_finished_and_proof() {
        let entry = create_entry(
            "wf-1".into(),
            "ship widget".into(),
            "feature".into(),
            "2026-05-16T10:00:00Z".into(),
        );
        assert_eq!(entry.status, "open");
        assert!(entry.finished_at.is_empty());
        assert!(entry.proof.is_empty());
    }

    #[test]
    fn close_entry_marks_status_closed_and_records_proof() {
        let opened = create_entry(
            "wf-1".into(),
            "ship widget".into(),
            "feature".into(),
            "2026-05-16T10:00:00Z".into(),
        );
        let closed = close_entry(opened, "2026-05-16T11:30:00Z".into(), "tests pass".into());
        assert_eq!(closed.status, "closed");
        assert_eq!(closed.finished_at, "2026-05-16T11:30:00Z");
        assert_eq!(closed.proof, "tests pass");
    }

    #[test]
    fn write_and_read_entry_round_trip_preserves_fields() {
        let claude_home = unique_temp_directory("round-trip");
        let entry = create_entry(
            "wf-2".into(),
            "fix race in queue \"drain\"\nwith newline".into(),
            "fix".into(),
            "2026-05-16T12:00:00Z".into(),
        );
        write_entry(&claude_home, &entry).expect("write entry");
        let loaded = read_entry(&claude_home, "wf-2")
            .expect("read entry")
            .expect("entry exists");
        assert_eq!(loaded, entry);
        let _ = fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn read_entry_returns_none_for_missing_id() {
        let claude_home = unique_temp_directory("missing");
        let result = read_entry(&claude_home, "wf-does-not-exist").expect("read");
        assert!(result.is_none());
        let _ = fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn list_entries_skips_non_json_files_and_sorts_by_started_at() {
        let claude_home = unique_temp_directory("list");
        let directory = ledger_directory(&claude_home);
        fs::create_dir_all(&directory).expect("ensure ledger directory");
        fs::write(directory.join("README.txt"), b"not an entry").expect("write decoy");
        let later = create_entry(
            "wf-later".into(),
            "later".into(),
            "feature".into(),
            "2026-05-16T15:00:00Z".into(),
        );
        let earlier = create_entry(
            "wf-earlier".into(),
            "earlier".into(),
            "fix".into(),
            "2026-05-16T09:00:00Z".into(),
        );
        write_entry(&claude_home, &later).expect("write later");
        write_entry(&claude_home, &earlier).expect("write earlier");
        let entries = list_entries(&claude_home).expect("list");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "wf-earlier");
        assert_eq!(entries[1].id, "wf-later");
        let _ = fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn list_entries_returns_empty_when_directory_missing() {
        let claude_home = unique_temp_directory("empty");
        let entries = list_entries(&claude_home).expect("list");
        assert!(entries.is_empty());
        let _ = fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn parse_entry_text_handles_html_safe_unicode_escapes() {
        let raw = r#"{
  "id": "wf-3",
  "request": "render <div>& friends",
  "preset": "feature",
  "status": "open",
  "startedAt": "2026-05-16T10:00:00Z",
  "finishedAt": "",
  "proof": ""
}
"#;
        let entry = parse_entry_text(raw).expect("parse");
        assert_eq!(entry.id, "wf-3");
        assert_eq!(entry.request, "render <div>& friends");
    }

    #[test]
    fn parse_entry_text_rejects_missing_id() {
        let raw = r#"{"request":"x","preset":"feature","status":"open","startedAt":"","finishedAt":"","proof":""}"#;
        assert!(parse_entry_text(raw).is_err());
    }

    #[test]
    fn allocate_unique_entry_id_appends_counter_on_collision() {
        let claude_home = unique_temp_directory("collision");
        let first_id = allocate_unique_entry_id(&claude_home, 100).expect("first id");
        assert_eq!(first_id, "wf-64");
        let entry = create_entry(
            first_id.clone(),
            "first".into(),
            "feature".into(),
            "2026-05-16T10:00:00Z".into(),
        );
        write_entry(&claude_home, &entry).expect("write first");
        let second_id = allocate_unique_entry_id(&claude_home, 100).expect("second id");
        assert_eq!(second_id, "wf-64-1");
        let _ = fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn format_timestamp_iso8601_known_unix_values() {
        assert_eq!(format_timestamp_iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(
            format_timestamp_iso8601(1_704_067_200_000),
            "2024-01-01T00:00:00Z"
        );
        assert_eq!(
            format_timestamp_iso8601(1_709_251_200_000),
            "2024-03-01T00:00:00Z"
        );
    }

    #[test]
    fn next_entry_id_uses_lowercase_hex_of_timestamp() {
        assert_eq!(next_entry_id(0), "wf-0");
        assert_eq!(next_entry_id(255), "wf-ff");
        assert_eq!(next_entry_id(1_747_400_000_000), "wf-196d9280200");
    }
}
