//! Purpose: Generic scoped JSON record store shared by the memory/orchestration command
//!   families (orchestration task, research-cache, loop-guard, agent-packets, agent-registry,
//!   entity, graph). One file per record under `<claude-home>/<group-path>/<id>.json`.
//! Caller: utility::memory handlers for the planned command families.
//! Dependencies: std::fs, std::path, crate::json::{write_indented, Value},
//!   crate::runtime::display_path, crate::utility::workflow_ledger::parse_object_of_strings.
//! Main Functions: RecordStore::new, write_record, read_record, list_records, delete_record,
//!   record_to_value, join_lines, split_lines.
//! Side Effects: Reads and writes flat-string JSON files under the scoped group directory.
//!   No global state.
//!
//! Storage shape: each record is an ordered list of (key, string-value) pairs. Multi-line
//! fields are joined with `\n` so the file stays parseable by the shared
//! `parse_object_of_strings` key=string reader. This mirrors `working_brief` and
//! `workflow_ledger` rather than introducing a second serialization concept.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::KeelError;
use crate::json::{write_indented, Value};
use crate::runtime::{display_path, safe_path_segment, write_text};
use crate::utility::workflow_ledger::parse_object_of_strings;

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

    /// Whether a record file already exists for `id`. Used by
    /// `allocate_unique_record_id` to avoid same-millisecond id collisions. An
    /// unsafe id can never exist as a stored record, so it reports `false`.
    #[cfg(test)]
    pub fn record_exists(&self, id: &str) -> bool {
        match self.validated_record_path(id) {
            Ok(path) => path.exists(),
            Err(_) => false,
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

/// Return an id that does not yet exist in `store`, starting from `base` and
/// appending `-1`, `-2`, ... on collision. Without this, two records created in
/// the same millisecond (a common case under fast test/CI execution) derive the
/// same `<prefix>-<ms>` id and the second write silently overwrites the first.
/// Mirrors the workflow ledger's `allocate_unique_entry_id`.
#[cfg(test)]
pub fn allocate_unique_record_id(store: &RecordStore, base: &str) -> String {
    if !store.record_exists(base) {
        return base.to_string();
    }
    for counter in 1..10_000u32 {
        let candidate = format!("{base}-{counter}");
        if !store.record_exists(&candidate) {
            return candidate;
        }
    }
    // Astronomically unlikely; fall back to the base rather than loop forever.
    base.to_string()
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
            assert!(
                !store.record_exists(evil),
                "record_exists must be false for unsafe id {evil:?}"
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
    fn allocate_unique_record_id_appends_counter_on_collision() {
        let home = temp_home("alloc-unique");
        let store = RecordStore::new(&home, "orchestration/tasks");
        // First allocation of an unused base returns it verbatim.
        let first = allocate_unique_record_id(&store, "task-abc");
        assert_eq!(first, "task-abc");
        store
            .write_record(&first, &vec![("id".into(), first.clone())])
            .expect("write first");
        // Same base now collides -> appends -1.
        let second = allocate_unique_record_id(&store, "task-abc");
        assert_eq!(second, "task-abc-1");
        store
            .write_record(&second, &vec![("id".into(), second.clone())])
            .expect("write second");
        // And again -> -2. This is exactly the same-millisecond case that made
        // two `task begin` calls collapse into one record on Linux/macOS CI.
        let third = allocate_unique_record_id(&store, "task-abc");
        assert_eq!(third, "task-abc-2");
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
}
