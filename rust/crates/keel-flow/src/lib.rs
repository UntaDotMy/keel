//! Purpose: Preserve-existing-flow evidence schema (the `Check` struct, validation, and JSON I/O).
//! Caller: `keel flow` lifecycle commands and review's brownfield ownership gate.
//! Dependencies: std::env, std::fs, std::io, std::path; serde and serde_json (workspace deps).
//! Main Functions: new_template_check, validate_check, finalize_check, write_check, load_check, resolve_artifact_path; LoadError carries the failure cases.
//! Side Effects: write/load read and write JSON files on disk; pure helpers do no I/O. Rust-native flow artifact validation.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_ARTIFACT_PATH: &str = "";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Check {
    #[serde(default)]
    pub version: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub task: String,
    #[serde(default, rename = "target_file")]
    pub target_file: String,
    #[serde(
        default,
        rename = "target_files",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub target_files: Vec<String>,
    #[serde(
        default,
        rename = "target_function",
        skip_serializing_if = "String::is_empty"
    )]
    pub target_function: String,
    #[serde(default, rename = "current_behavior_to_preserve")]
    pub current_behavior: String,
    #[serde(default, rename = "entry_point")]
    pub entry_point: String,
    #[serde(default)]
    pub producer: String,
    #[serde(default, rename = "source_of_truth")]
    pub source_of_truth: String,
    #[serde(default, rename = "storage_state_queue_owner")]
    pub storage_state_queue_owner: String,
    #[serde(default, rename = "side_effect_owner")]
    pub side_effect_owner: String,
    #[serde(default)]
    pub consumers: Vec<String>,
    #[serde(default, rename = "cleanup_recovery_path")]
    pub cleanup_recovery_path: String,
    #[serde(default, rename = "edit_boundary")]
    pub edit_boundary: String,
    #[serde(default, rename = "validation_needed")]
    pub validation_needed: Vec<String>,
    #[serde(default, rename = "validation_evidence")]
    pub validation_evidence: Vec<String>,
    #[serde(
        default,
        rename = "duplicate_owner_logic",
        skip_serializing_if = "is_false"
    )]
    pub duplicate_owner_logic: bool,
    #[serde(
        default,
        rename = "migration_approved",
        skip_serializing_if = "is_false"
    )]
    pub migration_approved: bool,
    #[serde(default, rename = "docs_only", skip_serializing_if = "is_false")]
    pub docs_only: bool,
    #[serde(default, rename = "formatting_only", skip_serializing_if = "is_false")]
    pub formatting_only: bool,
    #[serde(default, rename = "generated_only", skip_serializing_if = "is_false")]
    pub generated_only: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub greenfield: bool,
    #[serde(
        default,
        rename = "repository_head",
        skip_serializing_if = "String::is_empty"
    )]
    pub repository_head: String,
    #[serde(
        default,
        rename = "diff_fingerprint",
        skip_serializing_if = "String::is_empty"
    )]
    pub diff_fingerprint: String,
    #[serde(
        default,
        rename = "finalized_at",
        skip_serializing_if = "String::is_empty"
    )]
    pub finalized_at: String,
}

fn is_false(boolean_value: &bool) -> bool {
    !*boolean_value
}

fn io_error_message(source: &io::Error) -> String {
    let raw_message = source.to_string();
    if let Some((message_without_code, _)) = raw_message.rsplit_once(" (os error ") {
        return message_without_code.to_string();
    }
    raw_message
}

pub fn new_template_check(target_file: &str, target_function: &str) -> Check {
    Check {
        version: SCHEMA_VERSION,
        target_file: path_to_forward_slashes(target_file).trim().to_string(),
        target_files: vec![path_to_forward_slashes(target_file).trim().to_string()],
        target_function: target_function.trim().to_string(),
        consumers: Vec::new(),
        validation_needed: Vec::new(),
        validation_evidence: Vec::new(),
        ..Check::default()
    }
}

#[derive(Debug)]
pub enum LoadError {
    Read {
        artifact_path: PathBuf,
        source: io::Error,
    },
    Decode {
        artifact_path: PathBuf,
        reason: String,
    },
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Read {
                artifact_path,
                source,
            } => write!(
                formatter,
                "open {}: {}",
                artifact_path.display(),
                io_error_message(source)
            ),
            LoadError::Decode {
                artifact_path,
                reason,
            } => write!(
                formatter,
                "decode flow check artifact {}: {reason}",
                artifact_path.display()
            ),
        }
    }
}

impl std::error::Error for LoadError {}

pub fn load_check(repository_root: &Path, artifact_path: &str) -> Result<Check, LoadError> {
    let resolved_path = resolve_artifact_path(repository_root, artifact_path);
    let contents = fs::read(&resolved_path).map_err(|read_error| LoadError::Read {
        artifact_path: resolved_path.clone(),
        source: read_error,
    })?;
    let decoded: Check =
        serde_json::from_slice(&contents).map_err(|decode_error| LoadError::Decode {
            artifact_path: resolved_path.clone(),
            reason: decode_error.to_string(),
        })?;
    Ok(normalize_check(decoded))
}

pub fn write_check(
    repository_root: &Path,
    artifact_path: &str,
    check: Check,
) -> io::Result<PathBuf> {
    let resolved_path = resolve_artifact_path(repository_root, artifact_path);
    if let Some(parent_directory) = resolved_path.parent() {
        fs::create_dir_all(parent_directory)?;
    }
    let normalized = normalize_check(check);
    let mut encoded = serde_json::to_vec_pretty(&normalized)
        .expect("flow check serialization cannot fail for valid struct");
    encoded.push(b'\n');
    fs::write(&resolved_path, encoded)?;
    Ok(resolved_path)
}

pub fn resolve_artifact_path(repository_root: &Path, artifact_path: &str) -> PathBuf {
    let trimmed_artifact_path = artifact_path.trim();
    if trimmed_artifact_path.is_empty() {
        return default_workspace_artifact_path(repository_root);
    }
    let effective_path = trimmed_artifact_path;
    let candidate = PathBuf::from(effective_path);
    if candidate.is_absolute() {
        return clean_path(&candidate);
    }
    let mut joined_path = repository_root.to_path_buf();
    for segment in effective_path.split('/') {
        if segment.is_empty() {
            continue;
        }
        joined_path.push(segment);
    }
    joined_path
}

pub fn default_workspace_artifact_path(repository_root: &Path) -> PathBuf {
    let home = claude_home_directory();
    let key = workspace_key(repository_root);
    let preferred = home
        .join("memories")
        .join("workspaces")
        .join(&key)
        .join("flow")
        .join("flow-check.json");
    for legacy_key in workspace_key_aliases(repository_root).into_iter().skip(1) {
        let legacy = home
            .join("memories")
            .join("workspaces")
            .join(legacy_key)
            .join("flow")
            .join("flow-check.json");
        if legacy.is_file() && !preferred.is_file() {
            if let Some(parent) = preferred.parent() {
                if fs::create_dir_all(parent).is_ok() && fs::copy(&legacy, &preferred).is_ok() {
                    return preferred;
                }
            }
            return legacy;
        }
    }
    preferred
}

fn claude_home_directory() -> PathBuf {
    // Mirror the CLI home resolution: KEEL_HOME, CLAUDE_TARGET_OVERRIDE,
    // ~/.keel when it exists, legacy ~/.claude otherwise.
    for env_name in ["KEEL_HOME", "CLAUDE_TARGET_OVERRIDE"] {
        if let Ok(override_value) = env::var(env_name) {
            let trimmed = override_value.trim();
            if !trimmed.is_empty() {
                return clean_path(&PathBuf::from(trimmed));
            }
        }
    }
    let home = env::var("HOME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("USERPROFILE")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| ".".to_string());
    let home_path = clean_path(&PathBuf::from(home));
    let keel_home = home_path.join(".keel");
    if keel_home.is_dir() {
        return keel_home;
    }
    home_path.join(".claude")
}

/// Canonical directory key for every workspace-scoped Keel feature.
///
/// A hash is always present, including for short paths, so two paths that
/// sanitize to the same readable slug never share state. The complete key is
/// capped at 64 characters for portable filesystem components.
pub fn workspace_key(repository_root: &Path) -> String {
    let identity = workspace_identity(repository_root);
    workspace_key_from_identity(&identity)
}

fn workspace_key_from_identity(identity: &str) -> String {
    let slug = sanitize_workspace_identity(identity);
    let hash = fnv1a_64_hash(identity.as_bytes());
    let short_hash = &hash[..8];
    let prefix: String = slug.chars().take(55).collect();
    let prefix = prefix.trim_matches('-');
    if prefix.is_empty() {
        format!("ws-{short_hash}")
    } else {
        format!("{prefix}-{short_hash}")
    }
}

/// Previously shipped workspace keys, ordered from the most common to the
/// oldest. Callers use these only to copy missing state into the canonical
/// lane; sources are deliberately retained for recovery.
pub fn workspace_key_aliases(repository_root: &Path) -> Vec<String> {
    let mut unique = Vec::new();
    for identity in [
        workspace_identity(repository_root),
        raw_workspace_identity(repository_root),
    ] {
        let slug = sanitize_workspace_identity(&identity);
        let hash = fnv1a_64_hash(identity.as_bytes());
        let mut aliases = vec![workspace_key_from_identity(&identity), slug.clone()];
        if slug.chars().count() > 64 {
            let prefix: String = slug.chars().take(47).collect();
            aliases.push(format!("{prefix}-{hash}"));
        }
        let wide_prefix: String = slug.chars().take(100).collect();
        let wide_prefix = wide_prefix.trim_matches('-');
        aliases.push(if wide_prefix.is_empty() {
            format!("ws-{}", &hash[..8])
        } else {
            format!("{wide_prefix}-{}", &hash[..8])
        });
        for alias in aliases {
            if !unique.contains(&alias) {
                unique.push(alias);
            }
        }
    }
    unique
}

pub fn legacy_workspace_key(repository_root: &Path) -> String {
    sanitize_workspace_identity(&workspace_identity(repository_root))
}

fn sanitize_workspace_identity(identity: &str) -> String {
    let raw_key = path_to_forward_slashes(identity)
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    collapse_separator_runs(&raw_key)
}

fn workspace_identity(repository_root: &Path) -> String {
    let existing =
        fs::canonicalize(repository_root).unwrap_or_else(|_| clean_path(repository_root));
    #[cfg(windows)]
    let existing = windows_long_path(&existing).unwrap_or(existing);
    let identity = normalized_workspace_path(&existing);
    #[cfg(windows)]
    {
        identity.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    identity
}

fn raw_workspace_identity(repository_root: &Path) -> String {
    normalized_workspace_path(&clean_path(repository_root))
}

fn normalized_workspace_path(repository_root: &Path) -> String {
    let cleaned = clean_path(repository_root).to_string_lossy().to_string();
    #[cfg(windows)]
    {
        if let Some(rest) = cleaned.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{rest}");
        }
        if let Some(rest) = cleaned.strip_prefix(r"\\?\") {
            return rest.to_string();
        }
    }
    cleaned
}

#[cfg(windows)]
fn windows_long_path(path: &Path) -> Option<PathBuf> {
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLongPathNameW(short_path: *const u16, long_path: *mut u16, capacity: u32) -> u32;
    }

    let mut input: Vec<u16> = OsStr::new(path.as_os_str())
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let required = unsafe { GetLongPathNameW(input.as_mut_ptr(), std::ptr::null_mut(), 0) };
    if required == 0 {
        return None;
    }
    let mut output = vec![0u16; required as usize];
    let written = unsafe { GetLongPathNameW(input.as_ptr(), output.as_mut_ptr(), required) };
    if written == 0 || written >= required {
        return None;
    }
    output.truncate(written as usize);
    Some(PathBuf::from(OsString::from_wide(&output)))
}

fn collapse_separator_runs(value: &str) -> String {
    let mut collapsed = String::new();
    let mut previous_was_separator = false;
    for character in value.chars() {
        if character == '-' {
            if !previous_was_separator {
                collapsed.push(character);
            }
            previous_was_separator = true;
        } else {
            collapsed.push(character);
            previous_was_separator = false;
        }
    }
    collapsed
}

pub fn validate_check(check: Check) -> Vec<String> {
    let normalized = normalize_check(check);
    let mut validation_errors: Vec<String> = Vec::new();
    if normalized.version != SCHEMA_VERSION {
        validation_errors.push(format!("version must be {SCHEMA_VERSION}"));
    }
    if normalized.target_file.is_empty() {
        validation_errors.push("target_file is required".to_string());
    }
    if normalized.docs_only
        || normalized.formatting_only
        || normalized.generated_only
        || normalized.greenfield
    {
        return validation_errors;
    }
    let required_fields: [(&str, &str); 8] = [
        ("current_behavior_to_preserve", &normalized.current_behavior),
        ("entry_point", &normalized.entry_point),
        ("producer", &normalized.producer),
        ("source_of_truth", &normalized.source_of_truth),
        (
            "storage_state_queue_owner",
            &normalized.storage_state_queue_owner,
        ),
        ("side_effect_owner", &normalized.side_effect_owner),
        ("cleanup_recovery_path", &normalized.cleanup_recovery_path),
        ("edit_boundary", &normalized.edit_boundary),
    ];
    for (field_name, field_value) in required_fields {
        if field_value.trim().is_empty() {
            validation_errors.push(format!(
                "{field_name} is required for existing-source edits"
            ));
        }
    }
    if non_empty_strings(&normalized.consumers).is_empty() {
        validation_errors
            .push("consumers must name at least one consumer or Not found".to_string());
    }
    if non_empty_strings(&normalized.validation_needed).is_empty() {
        validation_errors
            .push("validation_needed must name at least one validation target".to_string());
    }
    if non_empty_strings(&normalized.validation_evidence).is_empty() {
        validation_errors
            .push("validation_evidence must name at least one completed evidence item".to_string());
    }
    if normalized.duplicate_owner_logic && !normalized.migration_approved {
        validation_errors
            .push("duplicate_owner_logic requires migration_approved evidence".to_string());
    }
    validation_errors
}

pub fn validate_finished_check(check: Check) -> Vec<String> {
    let normalized = normalize_check(check);
    let mut validation_errors = validate_check(normalized.clone());
    for (field_name, field_value) in [
        ("repository_head", normalized.repository_head.as_str()),
        ("diff_fingerprint", normalized.diff_fingerprint.as_str()),
        ("finalized_at", normalized.finalized_at.as_str()),
    ] {
        if field_value.is_empty() {
            validation_errors.push(format!("{field_name} is required after flow finish"));
        }
    }
    validation_errors
}

pub fn finalize_check(repository_root: &Path, mut check: Check) -> Result<Check, String> {
    let (repository_head, diff_fingerprint) = repository_state(repository_root)?;
    check.repository_head = repository_head;
    check.diff_fingerprint = diff_fingerprint;
    check.finalized_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("read system clock: {error}"))?
        .as_millis()
        .to_string();
    Ok(normalize_check(check))
}

pub fn repository_state(repository_root: &Path) -> Result<(String, String), String> {
    let head_output = Command::new("git")
        .arg("-C")
        .arg(repository_root)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|error| format!("run git rev-parse: {error}"))?;
    if !head_output.status.success() {
        return Err(format!(
            "git rev-parse HEAD failed: {}",
            String::from_utf8_lossy(&head_output.stderr).trim()
        ));
    }
    let diff_output = Command::new("git")
        .arg("-C")
        .arg(repository_root)
        .args(["diff", "--binary", "HEAD", "--", "."])
        .output()
        .map_err(|error| format!("run git diff: {error}"))?;
    if !diff_output.status.success() {
        return Err(format!(
            "git diff HEAD failed: {}",
            String::from_utf8_lossy(&diff_output.stderr).trim()
        ));
    }
    let repository_head = String::from_utf8_lossy(&head_output.stdout)
        .trim()
        .to_string();
    Ok((repository_head, fnv1a_64_hash(&diff_output.stdout)))
}

/// FNV-1a 64-bit hash for fast, non-cryptographic content fingerprinting.
/// Suitable for per-session diff fingerprinting; not collision-resistant.
fn fnv1a_64_hash(content: &[u8]) -> String {
    let mut hash: u64 = 14695981039346656037;
    for byte in content {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    format!("{hash:016x}")
}

fn normalize_check(mut check: Check) -> Check {
    if check.version == 0 {
        check.version = SCHEMA_VERSION;
    }
    check.target_file = path_to_forward_slashes(&check.target_file)
        .trim()
        .to_string();
    check.target_files = non_empty_strings(
        &check
            .target_files
            .iter()
            .map(|path| path_to_forward_slashes(path))
            .collect::<Vec<_>>(),
    );
    if check.target_files.is_empty() && !check.target_file.is_empty() {
        check.target_files.push(check.target_file.clone());
    }
    check.target_files.sort();
    check.target_files.dedup();
    check.target_function = check.target_function.trim().to_string();
    check.task = check.task.trim().to_string();
    check.current_behavior = check.current_behavior.trim().to_string();
    check.entry_point = check.entry_point.trim().to_string();
    check.producer = check.producer.trim().to_string();
    check.source_of_truth = check.source_of_truth.trim().to_string();
    check.storage_state_queue_owner = check.storage_state_queue_owner.trim().to_string();
    check.side_effect_owner = check.side_effect_owner.trim().to_string();
    check.cleanup_recovery_path = check.cleanup_recovery_path.trim().to_string();
    check.edit_boundary = check.edit_boundary.trim().to_string();
    check.consumers = non_empty_strings(&check.consumers);
    check.validation_needed = non_empty_strings(&check.validation_needed);
    check.validation_evidence = non_empty_strings(&check.validation_evidence);
    check.repository_head = check.repository_head.trim().to_string();
    check.diff_fingerprint = check.diff_fingerprint.trim().to_string();
    check.finalized_at = check.finalized_at.trim().to_string();
    check
}

fn non_empty_strings(input_values: &[String]) -> Vec<String> {
    let mut cleaned_values: Vec<String> = Vec::new();
    for value in input_values {
        let trimmed_value = value.trim();
        if !trimmed_value.is_empty() {
            cleaned_values.push(trimmed_value.to_string());
        }
    }
    cleaned_values
}

fn path_to_forward_slashes(raw_path: &str) -> String {
    raw_path.replace('\\', "/")
}

fn clean_path(raw_path: &Path) -> PathBuf {
    let original_string = raw_path.to_string_lossy();
    let is_windows_drive_absolute =
        cfg!(windows) && original_string.len() >= 3 && original_string.chars().nth(1) == Some(':');
    let preserve_leading_slash =
        original_string.starts_with('/') || original_string.starts_with('\\');
    let mut cleaned_segments: Vec<String> = Vec::new();
    let root_prefix: String = if is_windows_drive_absolute {
        original_string.chars().take(3).collect()
    } else if preserve_leading_slash {
        "/".to_string()
    } else {
        String::new()
    };
    let path_body = if !root_prefix.is_empty() {
        &original_string[root_prefix.len()..]
    } else {
        original_string.as_ref()
    };
    for raw_segment in path_body.split(['/', '\\']) {
        if raw_segment.is_empty() || raw_segment == "." {
            continue;
        }
        if raw_segment == ".." {
            if !cleaned_segments.is_empty()
                && cleaned_segments.last().map(String::as_str) != Some("..")
            {
                cleaned_segments.pop();
            } else if root_prefix.is_empty() {
                cleaned_segments.push(raw_segment.to_string());
            }
            continue;
        }
        cleaned_segments.push(raw_segment.to_string());
    }
    let joined_body = cleaned_segments.join(std::path::MAIN_SEPARATOR_STR);
    let mut result_string = String::new();
    if is_windows_drive_absolute {
        result_string.push_str(&root_prefix[..2]);
        result_string.push(std::path::MAIN_SEPARATOR);
    } else if preserve_leading_slash {
        result_string.push(std::path::MAIN_SEPARATOR);
    }
    result_string.push_str(&joined_body);
    if result_string.is_empty() {
        return PathBuf::from(".");
    }
    PathBuf::from(result_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn valid_flow_check_for_tests() -> Check {
        Check {
            version: SCHEMA_VERSION,
            target_file: "rust/crates/keel/src/example.rs".to_string(),
            target_function: "runExample".to_string(),
            current_behavior: "Existing command dispatch remains unchanged.".to_string(),
            entry_point: "Application::run".to_string(),
            producer: "runExample producer".to_string(),
            source_of_truth: "example owner".to_string(),
            storage_state_queue_owner: "Not found".to_string(),
            side_effect_owner: "standard output writer".to_string(),
            consumers: vec!["CLI caller".to_string()],
            cleanup_recovery_path: "returns exit code".to_string(),
            edit_boundary: "rust/crates/keel/src/example.rs only".to_string(),
            validation_needed: vec!["cargo test --workspace".to_string()],
            validation_evidence: vec!["cargo test --workspace passed".to_string()],
            ..Check::default()
        }
    }

    #[test]
    fn validate_check_requires_owner_path_and_validation_evidence() {
        let mut check = new_template_check("rust/crates/keel/src/example.rs", "runExample");
        check.version = SCHEMA_VERSION + 1;
        check.target_file = String::new();
        let validation_errors = validate_check(check);
        let joined_errors = validation_errors.join("\n");
        for expected_fragment in [
            "version must be",
            "target_file is required",
            "current_behavior_to_preserve is required",
            "entry_point is required",
            "producer is required",
            "source_of_truth is required",
            "storage_state_queue_owner is required",
            "side_effect_owner is required",
            "consumers must name",
            "validation_needed must name",
            "validation_evidence must name",
        ] {
            assert!(
                joined_errors.contains(expected_fragment),
                "missing fragment {expected_fragment:?} in {joined_errors:?}"
            );
        }
    }

    #[test]
    fn validate_check_blocks_duplicate_owner_logic_without_approval() {
        let mut check = valid_flow_check_for_tests();
        check.duplicate_owner_logic = true;
        let validation_errors = validate_check(check.clone());
        assert!(
            validation_errors.iter().any(|error_message| error_message
                .contains("duplicate_owner_logic requires migration_approved evidence")),
            "expected approval error, got {validation_errors:?}"
        );
        check.migration_approved = true;
        assert!(validate_check(check).is_empty());
    }

    #[test]
    fn write_and_load_check_round_trip() {
        let temporary_directory = tempdir_under("keel-flow-roundtrip");
        let check = valid_flow_check_for_tests();
        let written_path = write_check(&temporary_directory, "flow-check.json", check.clone())
            .expect("write check");
        let expected_path = temporary_directory.join("flow-check.json");
        assert_eq!(written_path, expected_path);
        let loaded_check = load_check(&temporary_directory, "flow-check.json").expect("load check");
        assert_eq!(loaded_check.target_file, check.target_file);
        assert_eq!(loaded_check.source_of_truth, check.source_of_truth);
        let _ = std::fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn validate_check_allows_explicit_exemptions() {
        let exemption_cases: [(&str, Check); 4] = [
            (
                "docs",
                Check {
                    target_file: "README.md".to_string(),
                    docs_only: true,
                    ..Check::default()
                },
            ),
            (
                "formatting",
                Check {
                    target_file: "rust/crates/keel/src/example.rs".to_string(),
                    formatting_only: true,
                    ..Check::default()
                },
            ),
            (
                "generated",
                Check {
                    target_file: "rust/crates/keel/src/generated.rs".to_string(),
                    generated_only: true,
                    ..Check::default()
                },
            ),
            (
                "greenfield",
                Check {
                    target_file: "rust/crates/keel/src/new.rs".to_string(),
                    greenfield: true,
                    ..Check::default()
                },
            ),
        ];
        for (case_name, exemption_check) in exemption_cases {
            let validation_errors = validate_check(exemption_check);
            assert!(
                validation_errors.is_empty(),
                "exemption {case_name} should pass, got {validation_errors:?}"
            );
        }
    }

    #[test]
    fn validate_finished_check_requires_diff_bound_metadata() {
        let check = valid_flow_check_for_tests();
        let errors = validate_finished_check(check.clone()).join("\n");
        assert!(errors.contains("repository_head"), "errors: {errors}");
        assert!(errors.contains("diff_fingerprint"), "errors: {errors}");
        assert!(errors.contains("finalized_at"), "errors: {errors}");

        let mut finished = check;
        finished.repository_head = "abc123".to_string();
        finished.diff_fingerprint = "def456".to_string();
        finished.finalized_at = "2026-08-31T00:00:00Z".to_string();
        assert!(validate_finished_check(finished).is_empty());
    }

    #[test]
    fn load_check_reports_missing_and_invalid_artifacts() {
        let temporary_directory = tempdir_under("keel-flow-invalid");
        match load_check(&temporary_directory, "flow-check.json") {
            Err(LoadError::Read {
                artifact_path,
                source,
            }) => {
                assert_eq!(artifact_path, temporary_directory.join("flow-check.json"));
                let rendered_error = format!(
                    "open {}: {}",
                    artifact_path.display(),
                    io_error_message(&source)
                );
                assert!(rendered_error.contains("flow-check.json"));
            }
            other_result => panic!("expected missing artifact error, got {other_result:?}"),
        }
        let artifact_path = resolve_artifact_path(&temporary_directory, "flow-check.json");
        fs::create_dir_all(artifact_path.parent().unwrap()).unwrap();
        fs::write(&artifact_path, b"{not-json").unwrap();
        match load_check(&temporary_directory, "flow-check.json") {
            Err(LoadError::Decode { reason, .. }) => {
                assert!(!reason.is_empty());
            }
            other_result => panic!("expected decode error, got {other_result:?}"),
        }
        let _ = std::fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn write_check_reports_filesystem_errors_and_resolves_paths() {
        let temporary_directory = tempdir_under("keel-flow-fs-errors");
        let file_backed_root = temporary_directory.join("not-a-directory");
        fs::write(&file_backed_root, b"file").unwrap();
        let write_error_for_file_backed = write_check(
            &file_backed_root,
            "flow-check.json",
            valid_flow_check_for_tests(),
        );
        assert!(write_error_for_file_backed.is_err());

        let directory_artifact_path = temporary_directory.join("directory-artifact");
        fs::create_dir_all(&directory_artifact_path).unwrap();
        let directory_artifact_string =
            directory_artifact_path.to_string_lossy().replace('\\', "/");
        let write_error_for_directory_artifact = write_check(
            &temporary_directory,
            &directory_artifact_string,
            valid_flow_check_for_tests(),
        );
        assert!(write_error_for_directory_artifact.is_err());

        let absolute_artifact_path = temporary_directory.join("absolute-flow.json");
        let absolute_artifact_string = absolute_artifact_path.to_string_lossy().to_string();
        let resolved_absolute =
            resolve_artifact_path(&temporary_directory, &absolute_artifact_string);
        assert_eq!(resolved_absolute, clean_path(&absolute_artifact_path));
        let _ = std::fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn default_artifact_path_uses_global_workspace_memory() {
        let _guard = ENV_LOCK.lock().expect("lock environment override");
        let temporary_directory = tempdir_under("keel-flow-default-global");
        let claude_home = temporary_directory.join("claude-home");
        let workspace_root = temporary_directory.join("workspace");
        let previous_override = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);

        let default_resolved_path = resolve_artifact_path(&workspace_root, DEFAULT_ARTIFACT_PATH);
        assert!(default_resolved_path.starts_with(&claude_home));
        assert!(!default_resolved_path.starts_with(&workspace_root));
        assert!(default_resolved_path.ends_with(Path::new("flow").join("flow-check.json")));

        if let Some(previous_value) = previous_override {
            std::env::set_var("CLAUDE_TARGET_OVERRIDE", previous_value);
        } else {
            std::env::remove_var("CLAUDE_TARGET_OVERRIDE");
        }
        let _ = std::fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn workspace_key_bounds_long_paths_without_collisions() {
        let shared = "C:/work/a-very-long-parent-segment-that-would-consume-the-entire-old-directory-key-before-the-project-name/";
        let first = workspace_key(Path::new(&format!("{shared}alpha")));
        let second = workspace_key(Path::new(&format!("{shared}beta")));
        assert_ne!(first, second);
        assert!(first.len() <= 64, "key={first}");
        assert!(second.len() <= 64, "key={second}");
    }

    #[test]
    fn workspace_key_is_always_hashed_even_for_short_paths() {
        let workspace = Path::new("C:/work/keel");
        let key = workspace_key(workspace);
        let identity = workspace_identity(workspace);
        let hash = fnv1a_64_hash(identity.as_bytes());

        assert!(
            key.ends_with(&hash[..8]),
            "canonical workspace lanes must always carry a path hash: {key}"
        );
        assert_ne!(key, legacy_workspace_key(workspace));
    }

    #[test]
    fn default_artifact_path_copies_legacy_flow_state_to_canonical_lane() {
        let _guard = ENV_LOCK.lock().expect("lock environment override");
        let temporary_directory = tempdir_under("keel-flow-migrate-legacy");
        let home = temporary_directory.join("home");
        let workspace = temporary_directory.join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        let previous_override = std::env::var("KEEL_HOME").ok();
        std::env::set_var("KEEL_HOME", &home);

        let legacy = home
            .join("memories")
            .join("workspaces")
            .join(legacy_workspace_key(&workspace))
            .join("flow")
            .join("flow-check.json");
        fs::create_dir_all(legacy.parent().expect("legacy parent")).expect("legacy directory");
        fs::write(&legacy, "legacy flow").expect("legacy artifact");

        let resolved = default_workspace_artifact_path(&workspace);
        assert!(resolved.is_file());
        assert_eq!(
            fs::read_to_string(&resolved).expect("migrated artifact"),
            "legacy flow"
        );
        assert_eq!(
            resolved
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .and_then(|value| value.to_str()),
            Some(workspace_key(&workspace).as_str())
        );
        assert!(
            legacy.is_file(),
            "migration must retain the recovery source"
        );

        if let Some(value) = previous_override {
            std::env::set_var("KEEL_HOME", value);
        } else {
            std::env::remove_var("KEEL_HOME");
        }
        let _ = fs::remove_dir_all(&temporary_directory);
    }

    fn tempdir_under(label: &str) -> PathBuf {
        let base_directory = std::env::temp_dir();
        let unique_suffix: u128 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let candidate = base_directory.join(format!("{label}-{unique_suffix}"));
        let _ = fs::create_dir_all(&candidate);
        candidate
    }
}
