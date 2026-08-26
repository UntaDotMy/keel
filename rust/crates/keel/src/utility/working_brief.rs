//! Purpose: Per-workspace working-brief storage backing the `keel memory working-brief` surface.
//! Caller: utility::memory::run_memory_command for the working-brief subcommand.
//! Dependencies: std::fs, std::path, crate::json::{write_indented, Value}, crate::runtime::display_path,
//!   crate::utility::record_store::parse_object_of_strings.
//! Main Functions: brief_directory, brief_path, write_brief, read_brief,
//!   create_brief, brief_to_value, parse_brief_text.
//! Side Effects: Reads and writes JSON files under `<claude-home>/working-briefs/`. No global state.
//!
//! Storage shape: one JSON file per brief at `<claude-home>/working-briefs/<id>.json`.
//! Files contain a flat object whose multi-line fields (constraints, acceptance criteria,
//! assumptions) are joined with `\n` so the parser stays the simple key=string parser shared
//! with the shared record-store parser.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::KeelError;
use crate::json::{write_indented, Value};
use crate::runtime::{display_path, safe_path_segment, write_text};
use crate::utility::record_store::parse_object_of_strings;

#[derive(Debug, Clone, PartialEq)]
pub struct Brief {
    pub id: String,
    pub request: String,
    pub constraints: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub assumptions: Vec<String>,
    /// Workspace this brief was written for, as a display path (e.g.
    /// `D:/Nasri/Project/keel`). Empty for legacy briefs written before
    /// the field existed and for writes where the cwd could not be resolved;
    /// consumers must treat empty as "unknown / applies anywhere" so older
    /// briefs never change behavior. Used by the working-brief gate to scope
    /// "did this workspace get a brief this session" instead of counting a brief
    /// written for an unrelated project.
    pub workspace: String,
    pub created_at: String,
    /// Completion-gate proof recorded on this brief by
    /// `memory completion-gate check --proof`. Empty until a gate check with a
    /// proof succeeds; replaces the deleted workflow ledger's proof column.
    pub proof: String,
}

pub fn brief_directory(claude_home: &Path) -> PathBuf {
    claude_home.join("working-briefs")
}

pub fn brief_path(claude_home: &Path, id: &str) -> PathBuf {
    brief_directory(claude_home).join(format!("{id}.json"))
}

/// Validate `id` as a single safe path segment before joining it into a brief
/// path. Brief ids arrive from CLI `--id` flags and MCP tool arguments; without
/// this guard `../../foo` or an absolute id would steer the `{id}.json` join
/// outside the working-briefs directory (arbitrary `.json` read/write). Guarding
/// inside write_brief/read_brief covers every caller by construction.
fn validated_brief_path(claude_home: &Path, id: &str) -> Result<PathBuf, KeelError> {
    match safe_path_segment(id) {
        Some(segment) => Ok(brief_path(claude_home, &segment)),
        None => Err(KeelError::Custom(format!(
            "invalid brief id {id:?}: must be a single safe path segment"
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn create_brief(
    id: String,
    request: String,
    constraints: Vec<String>,
    acceptance_criteria: Vec<String>,
    assumptions: Vec<String>,
    workspace: String,
    created_at: String,
) -> Brief {
    Brief {
        id,
        request,
        constraints,
        acceptance_criteria,
        assumptions,
        workspace,
        created_at,
        proof: String::new(),
    }
}

pub fn linear_issue_payload(brief: &Brief) -> Value {
    let description = brief.acceptance_criteria.join("\n");
    Value::Object(vec![
        ("title".into(), Value::String(brief.request.clone())),
        ("description".into(), Value::String(description)),
        ("keelBriefId".into(), Value::String(brief.id.clone())),
        ("workspace".into(), Value::String(brief.workspace.clone())),
    ])
}

pub fn brief_to_value(brief: &Brief) -> Value {
    Value::Object(vec![
        ("id".into(), Value::String(brief.id.clone())),
        ("request".into(), Value::String(brief.request.clone())),
        (
            "constraints".into(),
            Value::Array(
                brief
                    .constraints
                    .iter()
                    .map(|line| Value::String(line.clone()))
                    .collect(),
            ),
        ),
        (
            "acceptanceCriteria".into(),
            Value::Array(
                brief
                    .acceptance_criteria
                    .iter()
                    .map(|line| Value::String(line.clone()))
                    .collect(),
            ),
        ),
        (
            "assumptions".into(),
            Value::Array(
                brief
                    .assumptions
                    .iter()
                    .map(|line| Value::String(line.clone()))
                    .collect(),
            ),
        ),
        ("proof".into(), Value::String(brief.proof.clone())),
        ("workspace".into(), Value::String(brief.workspace.clone())),
        ("createdAt".into(), Value::String(brief.created_at.clone())),
    ])
}

fn brief_to_storage_value(brief: &Brief) -> Value {
    Value::Object(vec![
        ("id".into(), Value::String(brief.id.clone())),
        ("request".into(), Value::String(brief.request.clone())),
        (
            "constraints".into(),
            Value::String(brief.constraints.join("\n")),
        ),
        (
            "acceptanceCriteria".into(),
            Value::String(brief.acceptance_criteria.join("\n")),
        ),
        (
            "assumptions".into(),
            Value::String(brief.assumptions.join("\n")),
        ),
        ("workspace".into(), Value::String(brief.workspace.clone())),
        ("proof".into(), Value::String(brief.proof.clone())),
        ("createdAt".into(), Value::String(brief.created_at.clone())),
    ])
}

pub fn write_brief(claude_home: &Path, brief: &Brief) -> Result<PathBuf, KeelError> {
    let path = validated_brief_path(claude_home, &brief.id)?;
    let directory = brief_directory(claude_home);
    fs::create_dir_all(&directory)
        .map_err(|error| format!("create {}: {error}", display_path(&directory)))?;
    let mut serialized = Vec::<u8>::new();
    write_indented(&mut serialized, &brief_to_storage_value(brief))
        .map_err(|error| format!("serialize brief {}: {error}", brief.id))?;
    let text = String::from_utf8(serialized)
        .map_err(|error| format!("serialize brief {}: non-utf8 output: {error}", brief.id))?;
    // Atomic temp+fsync+rename so a crash never leaves a truncated brief that
    // would then poison list_briefs / the completion gate.
    write_text(&path, &text)?;
    Ok(path)
}

pub fn read_brief(claude_home: &Path, id: &str) -> Result<Option<Brief>, KeelError> {
    let path = validated_brief_path(claude_home, id)?;
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read {}: {error}", display_path(&path)).into()),
    };
    let brief = parse_brief_text(&text)
        .map_err(|error| format!("parse {}: {error}", display_path(&path)))?;
    Ok(Some(brief))
}

pub fn list_briefs(claude_home: &Path) -> Result<Vec<Brief>, KeelError> {
    let directory = brief_directory(claude_home);
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let read_iter = fs::read_dir(&directory)
        .map_err(|error| format!("read {}: {error}", display_path(&directory)))?;
    let mut briefs = Vec::new();
    for read_result in read_iter {
        let dir_entry =
            read_result.map_err(|error| format!("read {}: {error}", display_path(&directory)))?;
        let path = dir_entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        // Skip an unreadable/unparseable brief (e.g. a crash-truncated or
        // hand-edited file) rather than aborting the whole listing.
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("skip {}: {error}", display_path(&path));
                continue;
            }
        };
        match parse_brief_text(&text) {
            Ok(brief) => briefs.push(brief),
            Err(error) => {
                eprintln!("skip {}: {error}", display_path(&path));
                continue;
            }
        }
    }
    briefs.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(briefs)
}

pub fn parse_brief_text(text: &str) -> Result<Brief, KeelError> {
    let fields = parse_object_of_strings(text)?;
    let mut brief = Brief {
        id: String::new(),
        request: String::new(),
        constraints: Vec::new(),
        acceptance_criteria: Vec::new(),
        assumptions: Vec::new(),
        workspace: String::new(),
        proof: String::new(),
        created_at: String::new(),
    };
    for (key, value) in fields {
        match key.as_str() {
            "id" => brief.id = value,
            "request" => brief.request = value,
            "constraints" => brief.constraints = split_lines(&value),
            "acceptanceCriteria" => brief.acceptance_criteria = split_lines(&value),
            "assumptions" => brief.assumptions = split_lines(&value),
            "proof" => brief.proof = value,
            "workspace" => brief.workspace = value,
            "createdAt" => brief.created_at = value,
            _ => {}
        }
    }
    if brief.id.is_empty() {
        return Err("brief missing id field".into());
    }
    Ok(brief)
}

fn split_lines(joined: &str) -> Vec<String> {
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

    fn temp_claude_home(label: &str) -> PathBuf {
        let unique: u128 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let pid = std::process::id();
        let directory = std::env::temp_dir().join(format!("keel-brief-{label}-{pid}-{unique}"));
        fs::create_dir_all(&directory).expect("create tempdir");
        directory
    }

    #[test]
    fn write_then_read_round_trips_all_fields() {
        let claude_home = temp_claude_home("round-trip");
        let brief = create_brief(
            "wb-1".into(),
            "ship pagination".into(),
            vec!["must not break /users".into(), "no n+1 queries".into()],
            vec!["limit=20 default".into(), "expose nextCursor".into()],
            vec!["cursor encoding stays opaque".into()],
            "D:/Nasri/Project/example".into(),
            "2026-05-16T08:00:00Z".into(),
        );
        write_brief(&claude_home, &brief).expect("write brief");
        let round = read_brief(&claude_home, "wb-1")
            .expect("read brief")
            .expect("brief exists");
        assert_eq!(round, brief);
        assert_eq!(
            round.workspace, "D:/Nasri/Project/example",
            "workspace field must round-trip through storage"
        );
        let _ = fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn read_returns_none_for_missing_brief() {
        let claude_home = temp_claude_home("missing");
        let result = read_brief(&claude_home, "wb-missing").expect("read brief");
        assert!(result.is_none());
        let _ = fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn list_returns_briefs_in_creation_order() {
        let claude_home = temp_claude_home("list-order");
        write_brief(
            &claude_home,
            &create_brief(
                "wb-b".into(),
                "second".into(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                String::new(),
                "2026-05-16T08:01:00Z".into(),
            ),
        )
        .expect("write second");
        write_brief(
            &claude_home,
            &create_brief(
                "wb-a".into(),
                "first".into(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                String::new(),
                "2026-05-16T08:00:00Z".into(),
            ),
        )
        .expect("write first");
        let briefs = list_briefs(&claude_home).expect("list");
        assert_eq!(briefs.len(), 2);
        assert_eq!(briefs[0].id, "wb-a");
        assert_eq!(briefs[1].id, "wb-b");
        let _ = fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn empty_list_field_round_trips_as_empty_vec() {
        let claude_home = temp_claude_home("empty-fields");
        let brief = create_brief(
            "wb-empty".into(),
            "stub".into(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            String::new(),
            "2026-05-16T08:00:00Z".into(),
        );
        write_brief(&claude_home, &brief).expect("write");
        let round = read_brief(&claude_home, "wb-empty")
            .expect("read")
            .expect("brief exists");
        assert!(round.constraints.is_empty());
        assert!(round.acceptance_criteria.is_empty());
        assert!(round.assumptions.is_empty());
        let _ = fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn linear_issue_payload_uses_request_as_title() {
        let brief = create_brief(
            "wb-pm".into(),
            "Ship the stamp loop".into(),
            Vec::new(),
            vec!["gates pass".into()],
            Vec::new(),
            "D:/proj".into(),
            "2026-08-26T00:00:00Z".into(),
        );
        let payload = linear_issue_payload(&brief);
        let Value::Object(fields) = payload else {
            panic!("object");
        };
        let title = fields
            .iter()
            .find(|(key, _)| key == "title")
            .map(|(_, value)| value);
        match title {
            Some(Value::String(text)) => assert_eq!(text, "Ship the stamp loop"),
            other => panic!("title {other:?}"),
        }
    }
}
