//! Purpose: Generic scoped JSON record store shared by the memory command
//!   families (research-cache, loop-guard, agent-packets, agent-registry,
//!   entity, graph) plus the flat key=string parser and timestamp helpers the
//!   brief/record stores share (relocated here from the deleted workflow ledger).
//! Caller: utility::memory handlers, working_brief, memory_families, working_brief_cmd.
//! Dependencies: std::fs, std::path, std::time, crate::json::{write_indented, Value},
//!   crate::runtime::display_path.
//! Main Functions: RecordStore::new, write_record, read_record, list_records,
//!   delete_record, record_to_value, join_lines, split_lines,
//!   parse_object_of_strings, current_timestamp_millis, format_timestamp_iso8601.
//! Side Effects: Reads and writes flat-string JSON files under the scoped group directory.
//!   No global state.
//!
//! Storage shape: each record is an ordered list of (key, string-value) pairs. Multi-line
//! fields are joined with `\n` so the file stays parseable by the shared
//! `parse_object_of_strings` key=string reader. This mirrors `working_brief`
//! rather than introducing a second serialization concept.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::KeelError;
use crate::json::{write_indented, Value};
use crate::runtime::{display_path, safe_path_segment, write_text};

/// An ordered collection of string fields backing one stored record.
pub type Record = Vec<(String, String)>;

/// A scoped record collection rooted at `<claude_home>/<group_path>/`.
///
/// `group_path` may contain nested segments (for example `orchestration/tasks`);
/// each segment becomes a directory level so callers can group related
/// collections without inventing a second path scheme.
pub struct RecordStore {
    directory: PathBuf,
}

impl RecordStore {
    pub fn new(claude_home: &Path, group_path: &str) -> Self {
        let mut directory = claude_home.to_path_buf();
        for segment in group_path.split('/').filter(|segment| !segment.is_empty()) {
            directory.push(segment);
        }
        Self { directory }
    }

    /// Directory backing this collection. Test-only: production code addresses
    /// records by id via `record_path`/`read_record`/`list_records` rather than
    /// reaching for the raw directory, so this stays gated out of the shipped
    /// binary instead of carrying a dead-code allow.
    #[cfg(test)]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn record_path(&self, id: &str) -> PathBuf {
        self.directory.join(format!("{id}.json"))
    }

    /// Validate that `id` is a single safe path segment before it is joined into
    /// a record path. Record ids reach this store from CLI `--id` flags, MCP tool
    /// arguments, and workflow callers; without this guard a value like
    /// `../../foo` or an absolute path would steer the `{id}.json` join outside
    /// the store directory (arbitrary `.json` read/write/delete). Centralizing
    /// the check here means every id-addressed method is guarded by construction,
    /// rather than relying on each caller to sanitize first.
    fn validated_record_path(&self, id: &str) -> Result<PathBuf, KeelError> {
        match safe_path_segment(id) {
            Some(segment) => Ok(self.record_path(&segment)),
            None => Err(KeelError::Custom(format!(
                "invalid record id {id:?}: must be a single safe path segment"
            ))),
        }
    }

    pub fn write_record(&self, id: &str, fields: &Record) -> Result<PathBuf, KeelError> {
        let path = self.validated_record_path(id)?;
        fs::create_dir_all(&self.directory)
            .map_err(|error| format!("create {}: {error}", display_path(&self.directory)))?;
        let value = record_to_storage_value(fields);
        let mut serialized = Vec::<u8>::new();
        write_indented(&mut serialized, &value)
            .map_err(|error| format!("serialize record {id}: {error}"))?;
        let text = String::from_utf8(serialized)
            .map_err(|error| format!("serialize record {id}: non-utf8 output: {error}"))?;
        // Atomic temp+fsync+rename so a crash or concurrent reader never observes
        // a truncated record file (which would then poison `list_records`).
        write_text(&path, &text)?;
        Ok(path)
    }

    pub fn read_record(&self, id: &str) -> Result<Option<Record>, KeelError> {
        let path = self.validated_record_path(id)?;
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("read {}: {error}", display_path(&path)).into()),
        };
        let fields = parse_object_of_strings(&text)
            .map_err(|error| format!("parse {}: {error}", display_path(&path)))?;
        Ok(Some(fields))
    }

    /// Read every `*.json` record in the collection. Records are returned with
    /// their id (file stem) alongside the fields so callers can sort or filter
    /// without re-deriving the id. Returns an empty vec when the directory is
    /// absent so first-use is not an error.
    ///
    /// A single unreadable or unparseable file is skipped (with a stderr warning)
    /// rather than aborting the whole listing: a crash mid-write or a hand-edited
    /// bad file must not make `list`/cockpit/completion-gate fail wholesale until
    /// the user manually deletes it.
    pub fn list_records(&self) -> Result<Vec<(String, Record)>, KeelError> {
        if !self.directory.is_dir() {
            return Ok(Vec::new());
        }
        let read_iter = fs::read_dir(&self.directory)
            .map_err(|error| format!("read {}: {error}", display_path(&self.directory)))?;
        let mut records = Vec::new();
        for read_result in read_iter {
            let dir_entry = read_result
                .map_err(|error| format!("read {}: {error}", display_path(&self.directory)))?;
            let path = dir_entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let id = match path.file_stem().and_then(|stem| stem.to_str()) {
                Some(stem) => stem.to_string(),
                None => continue,
            };
            let text = match fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) => {
                    eprintln!("skip {}: {error}", display_path(&path));
                    continue;
                }
            };
            match parse_object_of_strings(&text) {
                Ok(fields) => records.push((id, fields)),
                Err(error) => {
                    eprintln!("skip {}: {error}", display_path(&path));
                    continue;
                }
            }
        }
        records.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(records)
    }

    /// Remove a record, returning whether it existed. Used by the learning
    /// loop's decay/prune step to drop auto-learned instincts whose pattern has
    /// aged out, and by family `forget`/`remove` actions.
    pub fn delete_record(&self, id: &str) -> Result<bool, KeelError> {
        let path = self.validated_record_path(id)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(format!("remove {}: {error}", display_path(&path)).into()),
        }
    }
}

/// Look up a single field value from a record by key.
pub fn field<'a>(record: &'a Record, key: &str) -> Option<&'a str> {
    record
        .iter()
        .find(|(field_key, _)| field_key == key)
        .map(|(_, value)| value.as_str())
}

/// Render a record as a JSON value for command output, splitting any field that
/// holds newline-joined lines back into an array so the output is structured.
/// Keys ending in `[]` are treated as list fields (the suffix is stripped).
pub fn record_to_value(fields: &Record) -> Value {
    Value::Object(
        fields
            .iter()
            .map(|(key, value)| {
                if let Some(base) = key.strip_suffix("[]") {
                    (
                        base.to_string(),
                        Value::Array(split_lines(value).into_iter().map(Value::String).collect()),
                    )
                } else {
                    (key.clone(), Value::String(value.clone()))
                }
            })
            .collect(),
    )
}

fn record_to_storage_value(fields: &Record) -> Value {
    Value::Object(
        fields
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// Join multi-line list fields with `\n` for flat storage.
pub fn join_lines(lines: &[String]) -> String {
    lines.join("\n")
}

/// Split a stored newline-joined field back into trimmed non-empty lines.
pub fn split_lines(joined: &str) -> Vec<String> {
    if joined.is_empty() {
        return Vec::new();
    }
    joined
        .split('\n')
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

/// Milliseconds since the Unix epoch. Shared id/clock source for the brief and
/// record stores (relocated from the deleted workflow ledger).
pub(crate) fn current_timestamp_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

/// Render epoch millis as `YYYY-MM-DDTHH:MM:SSZ` (UTC, no external time crate).
pub(crate) fn format_timestamp_iso8601(millis_since_epoch: u128) -> String {
    let total_seconds = (millis_since_epoch / 1000) as i64;
    let (year, month, day, hour, minute, second) = unix_seconds_to_civil(total_seconds);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Days-based civil-from-days algorithm (Howard Hinnant), unchanged from its
/// previous home in the workflow ledger.
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

/// Parse the flat key=string JSON dialect the brief and record stores write:
/// an object whose values are all string literals. Relocated verbatim from the
/// deleted workflow ledger so every flat store shares one parser.
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
                    // High surrogates require a following low surrogate escape.
                    // Combine the pair into one scalar.
                    if (0xD800..=0xDBFF).contains(&code_point) {
                        let high = code_point;
                        if *index + 10 >= bytes.len()
                            || bytes[*index + 5] != b'\\'
                            || bytes[*index + 6] != b'u'
                        {
                            return Err("lone high surrogate in \\u escape".into());
                        }
                        let low_hex = std::str::from_utf8(&bytes[*index + 7..*index + 11])
                            .map_err(|_| "non-utf8 in \\u low surrogate".to_string())?;
                        let low = u32::from_str_radix(low_hex, 16)
                            .map_err(|_| format!("invalid \\u hex: {low_hex}"))?;
                        if !(0xDC00..=0xDFFF).contains(&low) {
                            return Err("invalid low surrogate after high surrogate".into());
                        }
                        let scalar = 0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00);
                        let character = char::from_u32(scalar)
                            .ok_or_else(|| "surrogate pair out of range".to_string())?;
                        output.push(character);
                        // Advance past the low surrogate escape; shared increments
                        // handle the high surrogate prefix and hex digits.
                        *index += 6;
                    } else if (0xDC00..=0xDFFF).contains(&code_point) {
                        return Err("lone low surrogate in \\u escape".into());
                    } else if let Some(character) = char::from_u32(code_point) {
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

    fn temp_home(label: &str) -> PathBuf {
        let unique: u128 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let pid = std::process::id();
        let directory =
            std::env::temp_dir().join(format!("keel-recordstore-{label}-{pid}-{unique}"));
        fs::create_dir_all(&directory).expect("create tempdir");
        directory
    }

    #[test]
    fn write_then_read_round_trips_fields() {
        let home = temp_home("round-trip");
        let store = RecordStore::new(&home, "orchestration/tasks");
        let record: Record = vec![
            ("id".into(), "t-1".into()),
            ("phase".into(), "implement".into()),
            ("steps[]".into(), join_lines(&["a".into(), "b".into()])),
        ];
        store.write_record("t-1", &record).expect("write");
        let loaded = store
            .read_record("t-1")
            .expect("read")
            .expect("record exists");
        assert_eq!(field(&loaded, "phase"), Some("implement"));
        assert_eq!(field(&loaded, "steps[]"), Some("a\nb"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn rejects_traversal_and_absolute_ids() {
        let home = temp_home("traversal");
        let store = RecordStore::new(&home, "research-cache");
        for evil in [
            "../escape",
            "../../etc/passwd",
            "a/b",
            "a\\b",
            "/abs",
            "C:foo",
            "..",
        ] {
            assert!(
                store
                    .write_record(evil, &vec![("id".into(), "x".into())])
                    .is_err(),
                "write must reject id {evil:?}"
            );
            assert!(
                store.read_record(evil).is_err(),
                "read must reject id {evil:?}"
            );
            assert!(
                store.delete_record(evil).is_err(),
                "delete must reject id {evil:?}"
            );
        }
        // No file was created outside the store directory.
        assert!(!home.join("escape.json").exists());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn nested_group_path_creates_directory_levels() {
        let home = temp_home("nested");
        let store = RecordStore::new(&home, "memory/graph");
        store
            .write_record("e-1", &vec![("id".into(), "e-1".into())])
            .expect("write");
        assert!(home.join("memory/graph/e-1.json").is_file());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn read_missing_returns_none_and_list_missing_returns_empty() {
        let home = temp_home("missing");
        let store = RecordStore::new(&home, "research-cache");
        assert!(store.read_record("nope").expect("read").is_none());
        assert!(store.list_records().expect("list").is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn list_records_sorts_by_id_and_skips_non_json() {
        let home = temp_home("list");
        let store = RecordStore::new(&home, "loop-guard");
        store
            .write_record("b", &vec![("id".into(), "b".into())])
            .expect("write b");
        store
            .write_record("a", &vec![("id".into(), "a".into())])
            .expect("write a");
        fs::write(store.directory().join("notes.txt"), b"ignore").expect("decoy");
        let records = store.list_records().expect("list");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].0, "a");
        assert_eq!(records[1].0, "b");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn delete_record_reports_presence() {
        let home = temp_home("delete");
        let store = RecordStore::new(&home, "agent-packets");
        store
            .write_record("p-1", &vec![("id".into(), "p-1".into())])
            .expect("write");
        assert!(store.delete_record("p-1").expect("delete present"));
        assert!(!store.delete_record("p-1").expect("delete absent"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn record_to_value_expands_list_suffix_fields() {
        let record: Record = vec![
            ("id".into(), "x".into()),
            ("tags[]".into(), "one\ntwo".into()),
        ];
        let value = record_to_value(&record);
        let rendered = {
            let mut buffer = Vec::new();
            write_indented(&mut buffer, &value).unwrap();
            String::from_utf8(buffer).unwrap()
        };
        assert!(rendered.contains("\"tags\""));
        assert!(rendered.contains("\"one\""));
        assert!(rendered.contains("\"two\""));
        // The `[]` suffix must not leak into the rendered key.
        assert!(!rendered.contains("tags[]"));
    }

    #[test]
    fn format_timestamp_iso8601_renders_epoch() {
        assert_eq!(format_timestamp_iso8601(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn parse_object_of_strings_reads_flat_objects_only() {
        let fields = parse_object_of_strings("{\"id\": \"a\", \"note\": \"line\\nnext\"}")
            .expect("parse flat object");
        assert_eq!(
            fields,
            vec![
                ("id".to_string(), "a".to_string()),
                ("note".to_string(), "line\nnext".to_string()),
            ]
        );
        // Non-string values and non-objects are outside the dialect.
        assert!(parse_object_of_strings("{\"n\": 1}").is_err());
        assert!(parse_object_of_strings("[]").is_err());
    }
}
