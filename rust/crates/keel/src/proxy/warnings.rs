//! Purpose: Parse captured verification diagnostics and persist warning workflow state.
//! Caller: proxy::run, warning CLI, review gates, and completion gates.
//! Dependencies: CommandAst, workspace-key memory lanes, serde, chrono, and atomic runtime writes.
//! Main Functions: parse_diagnostics, reconcile_capture, run_warn_command, warning_gate.
//! Side Effects: Writes append-only warning ledger events and linked task-ticket state.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::args::FlagSet;
use crate::proxy::command_ast::CommandAst;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Diagnostic {
    pub severity: String,
    pub file: Option<String>,
    pub line: Option<u64>,
    pub col: Option<u64>,
    pub message: String,
    pub rule: Option<String>,
    pub source_command: String,
    pub captured_at: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WarningStatus {
    Baseline,
    Open,
    Resolved,
    Waived,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WarningSummary {
    pub open: usize,
    pub new: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WarningGateSummary {
    pub blocking: bool,
    pub open: usize,
    pub baseline: usize,
    pub waived: usize,
    pub resolved: usize,
    pub details: String,
}

const LEDGER_SCHEMA_VERSION: u64 = 1;
const MAX_LEDGER_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LedgerRecord {
    schema_version: u64,
    event: String,
    family: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<WarningStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostic: Option<Diagnostic>,
    source_command: String,
    captured_at: String,
    raw_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    waiver_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    waiver_expires_at: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct CurrentWarning {
    pub diagnostic: Diagnostic,
    pub status: WarningStatus,
    pub family: String,
    pub waiver_reason: Option<String>,
    pub waiver_expires_at: Option<String>,
}

pub(crate) fn parse_diagnostics(
    ast: &CommandAst,
    stdout: &[u8],
    stderr: &[u8],
    source_command: &str,
    captured_at: &str,
) -> Vec<Diagnostic> {
    let Some(family) = ast.verification_family() else {
        return Vec::new();
    };
    let merged = format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    );
    let clean = crate::adapters::common::strip_ansi_escape(&merged);
    let mut diagnostics = match family {
        "dart" => parse_dart(&clean, source_command, captured_at),
        "rust" => parse_rust(&clean, source_command, captured_at),
        "typescript" => parse_typescript(&clean, source_command, captured_at),
        "cpp" => parse_cpp(&clean, source_command, captured_at),
        "python" => parse_python(&clean, source_command, captured_at),
        "package" => parse_package(&clean, source_command, captured_at),
        "dotnet" => parse_dotnet(&clean, source_command, captured_at),
        _ => Vec::new(),
    };
    diagnostics.sort_by(|left, right| left.fingerprint.cmp(&right.fingerprint));
    diagnostics.dedup_by(|left, right| left.fingerprint == right.fingerprint);
    diagnostics
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn reconcile_capture(
    keel_home: &Path,
    workspace_root: &Path,
    ast: &CommandAst,
    stdout: &[u8],
    stderr: &[u8],
    exit_code: i32,
    source_command: &str,
    captured_at: &str,
    raw_id: &str,
) -> Result<WarningSummary, String> {
    let Some(family) = ast.verification_family() else {
        return Ok(WarningSummary { open: 0, new: 0 });
    };
    let ledger_path = warning_ledger_path(keel_home, workspace_root);
    let mut records = load_ledger(&ledger_path)?;
    let current = current_warnings_from_records(&records, captured_at)?;
    let initialized = records.iter().any(|record| record.family == family);
    let observed = parse_diagnostics(ast, stdout, stderr, source_command, captured_at);
    let observed_fingerprints: BTreeSet<String> = observed
        .iter()
        .map(|diagnostic| diagnostic.fingerprint.clone())
        .collect();
    let mut appended = Vec::new();
    let mut newly_opened = 0usize;
    let mut opened_links = Vec::new();

    for diagnostic in observed {
        let next_status = match current.get(&diagnostic.fingerprint) {
            None if initialized => Some(WarningStatus::Open),
            None => Some(WarningStatus::Baseline),
            Some(existing) if existing.status == WarningStatus::Resolved => {
                Some(WarningStatus::Open)
            }
            Some(existing) if existing.status == WarningStatus::Waived => {
                if waiver_is_expired(existing.waiver_expires_at.as_deref(), captured_at)? {
                    Some(WarningStatus::Open)
                } else {
                    None
                }
            }
            Some(_) => None,
        };
        if let Some(status) = next_status {
            if status == WarningStatus::Open {
                newly_opened += 1;
                opened_links.push(crate::utility::task_ticket::WarningTicketLink {
                    fingerprint: diagnostic.fingerprint.clone(),
                    description: format!(
                        "Resolve captured warning {} at {}.",
                        diagnostic.fingerprint,
                        warning_location(&diagnostic)
                    ),
                });
            }
            appended.push(diagnostic_record(
                family,
                status,
                diagnostic,
                source_command,
                captured_at,
                raw_id,
            ));
        }
    }

    if exit_code == 0 {
        for warning in current.values().filter(|warning| warning.family == family) {
            if warning.status != WarningStatus::Resolved
                && warning.status != WarningStatus::Waived
                && !observed_fingerprints.contains(&warning.diagnostic.fingerprint)
            {
                appended.push(diagnostic_record(
                    family,
                    WarningStatus::Resolved,
                    warning.diagnostic.clone(),
                    source_command,
                    captured_at,
                    raw_id,
                ));
            }
        }
    }

    appended.push(LedgerRecord {
        schema_version: LEDGER_SCHEMA_VERSION,
        event: "scan".to_string(),
        family: family.to_string(),
        status: None,
        diagnostic: None,
        source_command: source_command.to_string(),
        captured_at: captured_at.to_string(),
        raw_id: raw_id.to_string(),
        waiver_reason: None,
        waiver_expires_at: None,
    });
    append_ledger(&ledger_path, &appended)?;
    records.extend(appended);
    if !opened_links.is_empty() {
        let _ = crate::utility::task_ticket::link_open_warning_subtasks(
            keel_home,
            workspace_root,
            &opened_links,
        );
    }
    let updated = current_warnings_from_records(&records, captured_at)?;
    Ok(WarningSummary {
        open: updated
            .values()
            .filter(|warning| warning.status == WarningStatus::Open)
            .count(),
        new: newly_opened,
    })
}

pub(crate) fn warning_pointer(summary: &WarningSummary) -> Option<String> {
    (summary.new > 0).then(|| {
        format!(
            "warnings: {} open ({} new) — run `keel warn list`",
            summary.open, summary.new
        )
    })
}

pub(crate) fn warning_ledger_path(keel_home: &Path, workspace_root: &Path) -> PathBuf {
    keel_home
        .join("memories")
        .join("workspaces")
        .join(crate::utility::system_map::workspace_key(
            &workspace_root.to_string_lossy(),
        ))
        .join("warnings")
        .join("ledger.jsonl")
}

pub(crate) fn current_warnings(
    keel_home: &Path,
    workspace_root: &Path,
    now: &str,
) -> Result<Vec<CurrentWarning>, String> {
    let records = load_ledger(&warning_ledger_path(keel_home, workspace_root))?;
    Ok(current_warnings_from_records(&records, now)?
        .into_values()
        .collect())
}

pub(crate) fn waive_warning(
    keel_home: &Path,
    workspace_root: &Path,
    fingerprint: &str,
    reason: &str,
    expires: &str,
    now: &str,
) -> Result<CurrentWarning, String> {
    let reason = reason.trim();
    if reason.chars().count() < 10 {
        return Err("warning waiver reason must contain at least 10 characters".to_string());
    }
    let days = expires
        .strip_suffix('d')
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|days| (1..=3650).contains(days))
        .ok_or_else(|| {
            "warning waiver expiry must be a positive duration such as 30d (maximum 3650d)"
                .to_string()
        })?;
    let now_value = DateTime::parse_from_rfc3339(now)
        .map_err(|error| format!("invalid warning timestamp {now}: {error}"))?;
    let expires_at = (now_value + Duration::days(days))
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Secs, true);
    let ledger_path = warning_ledger_path(keel_home, workspace_root);
    let records = load_ledger(&ledger_path)?;
    let current = current_warnings_from_records(&records, now)?;
    let warning = current
        .get(fingerprint)
        .ok_or_else(|| format!("warning fingerprint not found: {fingerprint}"))?;
    if warning.status == WarningStatus::Resolved {
        return Err(format!(
            "warning fingerprint is already resolved: {fingerprint}"
        ));
    }
    let record = LedgerRecord {
        schema_version: LEDGER_SCHEMA_VERSION,
        event: "waiver".to_string(),
        family: warning.family.clone(),
        status: Some(WarningStatus::Waived),
        diagnostic: Some(warning.diagnostic.clone()),
        source_command: "keel warn waive".to_string(),
        captured_at: now.to_string(),
        raw_id: String::new(),
        waiver_reason: Some(reason.to_string()),
        waiver_expires_at: Some(expires_at.clone()),
    };
    append_ledger(&ledger_path, &[record])?;
    Ok(CurrentWarning {
        diagnostic: warning.diagnostic.clone(),
        status: WarningStatus::Waived,
        family: warning.family.clone(),
        waiver_reason: Some(reason.to_string()),
        waiver_expires_at: Some(expires_at),
    })
}

pub(crate) fn warning_gate(
    keel_home: &Path,
    workspace_root: &Path,
    now: &str,
) -> Result<WarningGateSummary, String> {
    let warnings = current_warnings(keel_home, workspace_root, now)?;
    let open = warnings
        .iter()
        .filter(|warning| warning.status == WarningStatus::Open)
        .count();
    let baseline = warnings
        .iter()
        .filter(|warning| warning.status == WarningStatus::Baseline)
        .count();
    let waived = warnings
        .iter()
        .filter(|warning| warning.status == WarningStatus::Waived)
        .count();
    let resolved = warnings
        .iter()
        .filter(|warning| warning.status == WarningStatus::Resolved)
        .count();
    let waiver_details = warnings
        .iter()
        .filter(|warning| warning.status == WarningStatus::Waived)
        .map(|warning| {
            format!(
                "{} until {}",
                warning.diagnostic.fingerprint,
                warning
                    .waiver_expires_at
                    .as_deref()
                    .unwrap_or("missing-expiry")
            )
        })
        .collect::<Vec<_>>();
    let details = if waiver_details.is_empty() {
        format!("open={open} baseline={baseline} waived={waived} resolved={resolved}")
    } else {
        format!(
            "open={open} baseline={baseline} waived={waived} resolved={resolved}; waivers: {}",
            waiver_details.join(", ")
        )
    };
    Ok(WarningGateSummary {
        blocking: open > 0,
        open,
        baseline,
        waived,
        resolved,
        details,
    })
}

pub fn run_warn_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || matches!(arguments[0].as_str(), "help" | "--help" | "-h") {
        let _ = writeln!(
            standard_output,
            "Usage: keel warn list [--repo-root <path>] [--claude-home <path>] [--json] | waive <fingerprint> --reason <text> --expires <Nd>"
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "list" => run_warn_list(&arguments[1..], standard_output, standard_error),
        "waive" => run_warn_waive(&arguments[1..], standard_output, standard_error),
        other => {
            let _ = writeln!(standard_error, "Unknown warn command: {other}");
            1
        }
    }
}

fn warning_cli_roots(flags: &FlagSet) -> Result<(PathBuf, PathBuf), String> {
    let home = crate::runtime::resolve_claude_home(flags.string_value("claude-home"))?;
    let requested = crate::runtime::resolve_repository_root(flags.string_value("repo-root"))?;
    Ok((home, warning_workspace_root(&requested)))
}

fn warning_flags(name: &str) -> FlagSet {
    let mut flags = FlagSet::new(name);
    flags.string_flag("repo-root", "");
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    flags
}

fn run_warn_list(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = warning_flags("warn list");
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    if !flags.positional.is_empty() {
        let _ = writeln!(standard_error, "warn list accepts no positional arguments");
        return 1;
    }
    let (home, workspace) = match warning_cli_roots(&flags) {
        Ok(roots) => roots,
        Err(error) => {
            let _ = writeln!(standard_error, "warn list: {error}");
            return 1;
        }
    };
    let now = now_timestamp();
    let warnings = match current_warnings(&home, &workspace, &now) {
        Ok(warnings) => warnings,
        Err(error) => {
            let _ = writeln!(standard_error, "warn list: {error}");
            return 1;
        }
    };
    let gate = match warning_gate(&home, &workspace, &now) {
        Ok(gate) => gate,
        Err(error) => {
            let _ = writeln!(standard_error, "warn list: {error}");
            return 1;
        }
    };
    if flags.bool_value("json") {
        let rows: Vec<serde_json::Value> = warnings
            .iter()
            .map(|warning| {
                serde_json::json!({
                    "status": status_name(&warning.status),
                    "family": warning.family,
                    "diagnostic": warning.diagnostic,
                    "waiver_reason": warning.waiver_reason,
                    "waiver_expires_at": warning.waiver_expires_at,
                })
            })
            .collect();
        let payload = serde_json::json!({
            "ok": !gate.blocking,
            "open": gate.open,
            "baseline": gate.baseline,
            "waived": gate.waived,
            "resolved": gate.resolved,
            "ledger": crate::runtime::display_path(&warning_ledger_path(&home, &workspace)),
            "warnings": rows,
        });
        let _ = writeln!(
            standard_output,
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_default()
        );
        return 0;
    }
    let _ = writeln!(
        standard_output,
        "warning ledger: {} finding(s); {}",
        warnings.len(),
        gate.details
    );
    for warning in warnings {
        let location = warning_location(&warning.diagnostic);
        let rule = warning
            .diagnostic
            .rule
            .as_deref()
            .map(|rule| format!(" [{rule}]"))
            .unwrap_or_default();
        let _ = writeln!(
            standard_output,
            "[{}] {} {}{}: {}",
            status_name(&warning.status),
            warning.diagnostic.fingerprint,
            location,
            rule,
            warning.diagnostic.message
        );
        let _ = writeln!(
            standard_output,
            "  command: {} | captured: {}",
            warning.diagnostic.source_command, warning.diagnostic.captured_at
        );
        if let Some(reason) = warning.waiver_reason {
            let _ = writeln!(
                standard_output,
                "  waiver: {reason} | expires: {}",
                warning.waiver_expires_at.as_deref().unwrap_or("missing")
            );
        }
    }
    0
}

fn run_warn_waive(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = warning_flags("warn waive");
    flags.string_flag("reason", "");
    flags.string_flag("expires", "");
    let (leading_fingerprint, flag_arguments) = arguments
        .first()
        .filter(|value| !value.starts_with('-'))
        .map_or((None, arguments), |fingerprint| {
            (Some(fingerprint.clone()), &arguments[1..])
        });
    if let Err(error) = flags.parse(flag_arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let had_leading_fingerprint = leading_fingerprint.is_some();
    let fingerprint = leading_fingerprint
        .or_else(|| (flags.positional.len() == 1).then(|| flags.positional[0].clone()));
    if fingerprint.is_none() || (!flags.positional.is_empty() && had_leading_fingerprint) {
        let _ = writeln!(
            standard_error,
            "warn waive requires exactly one warning fingerprint"
        );
        return 1;
    }
    let (home, workspace) = match warning_cli_roots(&flags) {
        Ok(roots) => roots,
        Err(error) => {
            let _ = writeln!(standard_error, "warn waive: {error}");
            return 1;
        }
    };
    let fingerprint = fingerprint.unwrap();
    let waived = match waive_warning(
        &home,
        &workspace,
        &fingerprint,
        flags.string_value("reason"),
        flags.string_value("expires"),
        &now_timestamp(),
    ) {
        Ok(warning) => warning,
        Err(error) => {
            let _ = writeln!(standard_error, "warn waive: {error}");
            return 1;
        }
    };
    if flags.bool_value("json") {
        let payload = serde_json::json!({
            "ok": true,
            "fingerprint": waived.diagnostic.fingerprint,
            "status": "waived",
            "reason": waived.waiver_reason,
            "expires_at": waived.waiver_expires_at,
        });
        let _ = writeln!(
            standard_output,
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_default()
        );
    } else {
        let _ = writeln!(
            standard_output,
            "warning waived: {} until {}",
            waived.diagnostic.fingerprint,
            waived.waiver_expires_at.as_deref().unwrap_or("missing")
        );
    }
    0
}

fn status_name(status: &WarningStatus) -> &'static str {
    match status {
        WarningStatus::Baseline => "baseline",
        WarningStatus::Open => "open",
        WarningStatus::Resolved => "resolved",
        WarningStatus::Waived => "waived",
    }
}

fn warning_location(diagnostic: &Diagnostic) -> String {
    match (&diagnostic.file, diagnostic.line, diagnostic.col) {
        (Some(file), Some(line), Some(col)) => format!("{file}:{line}:{col}"),
        (Some(file), Some(line), None) => format!("{file}:{line}"),
        (Some(file), None, None) => file.clone(),
        _ => "<no location>".to_string(),
    }
}

fn warning_workspace_root(candidate: &Path) -> PathBuf {
    let arguments = ["rev-parse".to_string(), "--show-toplevel".to_string()];
    crate::runtime::run_command("git", &arguments, Some(candidate))
        .ok()
        .filter(|result| result.code == 0)
        .and_then(|result| {
            let path = String::from_utf8_lossy(&result.stdout).trim().to_string();
            (!path.is_empty()).then(|| PathBuf::from(path))
        })
        .unwrap_or_else(|| candidate.to_path_buf())
}

fn now_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn diagnostic_record(
    family: &str,
    status: WarningStatus,
    diagnostic: Diagnostic,
    source_command: &str,
    captured_at: &str,
    raw_id: &str,
) -> LedgerRecord {
    LedgerRecord {
        schema_version: LEDGER_SCHEMA_VERSION,
        event: "diagnostic".to_string(),
        family: family.to_string(),
        status: Some(status),
        diagnostic: Some(diagnostic),
        source_command: source_command.to_string(),
        captured_at: captured_at.to_string(),
        raw_id: raw_id.to_string(),
        waiver_reason: None,
        waiver_expires_at: None,
    }
}

fn load_ledger(path: &Path) -> Result<Vec<LedgerRecord>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let metadata = fs::metadata(path)
        .map_err(|error| format!("warning ledger metadata {}: {error}", path.display()))?;
    if metadata.len() > MAX_LEDGER_BYTES {
        return Err(format!(
            "warning ledger {} exceeds {} bytes",
            path.display(),
            MAX_LEDGER_BYTES
        ));
    }
    let text = fs::read_to_string(path)
        .map_err(|error| format!("read warning ledger {}: {error}", path.display()))?;
    let mut records = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: LedgerRecord = serde_json::from_str(line).map_err(|error| {
            format!(
                "parse warning ledger {} line {}: {error}",
                path.display(),
                index + 1
            )
        })?;
        if record.schema_version != LEDGER_SCHEMA_VERSION {
            return Err(format!(
                "warning ledger {} line {} has unsupported schema_version {}",
                path.display(),
                index + 1,
                record.schema_version
            ));
        }
        records.push(record);
    }
    Ok(records)
}

fn append_ledger(path: &Path, records: &[LedgerRecord]) -> Result<(), String> {
    if records.is_empty() {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("warning ledger has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "create warning ledger directory {}: {error}",
            parent.display()
        )
    })?;
    let mut payload = Vec::new();
    for record in records {
        serde_json::to_writer(&mut payload, record)
            .map_err(|error| format!("serialize warning ledger record: {error}"))?;
        payload.push(b'\n');
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("open warning ledger {}: {error}", path.display()))?;
    file.write_all(&payload)
        .map_err(|error| format!("append warning ledger {}: {error}", path.display()))
}

fn current_warnings_from_records(
    records: &[LedgerRecord],
    now: &str,
) -> Result<BTreeMap<String, CurrentWarning>, String> {
    let mut current = BTreeMap::new();
    for record in records {
        let (Some(diagnostic), Some(status)) = (&record.diagnostic, &record.status) else {
            continue;
        };
        current.insert(
            diagnostic.fingerprint.clone(),
            CurrentWarning {
                diagnostic: diagnostic.clone(),
                status: status.clone(),
                family: record.family.clone(),
                waiver_reason: record.waiver_reason.clone(),
                waiver_expires_at: record.waiver_expires_at.clone(),
            },
        );
    }
    for warning in current.values_mut() {
        if warning.status == WarningStatus::Waived
            && waiver_is_expired(warning.waiver_expires_at.as_deref(), now)?
        {
            warning.status = WarningStatus::Open;
        }
    }
    Ok(current)
}

fn waiver_is_expired(expires_at: Option<&str>, now: &str) -> Result<bool, String> {
    let Some(expires_at) = expires_at else {
        return Ok(true);
    };
    let expiry = DateTime::parse_from_rfc3339(expires_at)
        .map_err(|error| format!("invalid warning waiver expiry {expires_at}: {error}"))?;
    let now = DateTime::parse_from_rfc3339(now)
        .map_err(|error| format!("invalid warning timestamp {now}: {error}"))?;
    Ok(expiry <= now)
}

fn parse_dart(text: &str, command: &str, captured_at: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let parts: Vec<&str> = line.split(" • ").collect();
        if parts.len() >= 4 && is_diagnostic_severity(parts[0]) {
            if let Some((file, row, col)) = parse_colon_location(parts[2]) {
                diagnostics.push(make_diagnostic(
                    parts[0],
                    Some(file),
                    Some(row),
                    Some(col),
                    parts[1],
                    Some(parts[3].trim().to_string()),
                    command,
                    captured_at,
                ));
                continue;
            }
        }
        let machine: Vec<&str> = line.splitn(8, '|').collect();
        if machine.len() == 8
            && is_diagnostic_severity(machine[0])
            && machine[4].parse::<u64>().is_ok()
            && machine[5].parse::<u64>().is_ok()
        {
            diagnostics.push(make_diagnostic(
                machine[0],
                Some(machine[3].to_string()),
                machine[4].parse().ok(),
                machine[5].parse().ok(),
                machine[7],
                Some(machine[2].to_string()),
                command,
                captured_at,
            ));
        }
    }
    diagnostics
}

fn parse_rust(text: &str, command: &str, captured_at: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let span = Regex::new(r"^\s*-->\s+(.+):(\d+):(\d+)\s*$").unwrap();
    let lint = Regex::new(r"#\[warn\(([^)]+)\)\]").unwrap();
    for (index, line) in lines.iter().enumerate() {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) {
            if value.get("reason").and_then(serde_json::Value::as_str) == Some("compiler-message") {
                if let Some(message) = value.get("message") {
                    let severity = message
                        .get("level")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default();
                    if is_diagnostic_severity(severity) {
                        let primary = message
                            .get("spans")
                            .and_then(serde_json::Value::as_array)
                            .and_then(|spans| {
                                spans.iter().find(|span| {
                                    span.get("is_primary").and_then(serde_json::Value::as_bool)
                                        == Some(true)
                                })
                            });
                        diagnostics.push(make_diagnostic(
                            severity,
                            primary
                                .and_then(|span| span.get("file_name"))
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string),
                            primary
                                .and_then(|span| span.get("line_start"))
                                .and_then(serde_json::Value::as_u64),
                            primary
                                .and_then(|span| span.get("column_start"))
                                .and_then(serde_json::Value::as_u64),
                            message
                                .get("message")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default(),
                            message
                                .get("code")
                                .and_then(|code| code.get("code"))
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string),
                            command,
                            captured_at,
                        ));
                    }
                }
            }
        }
        let trimmed = line.trim_start();
        let Some((severity, message)) = ["warning", "error"].into_iter().find_map(|severity| {
            trimmed
                .strip_prefix(&format!("{severity}:"))
                .map(|message| (severity, message.trim()))
        }) else {
            continue;
        };
        let following = &lines[index + 1..lines.len().min(index + 10)];
        let location = following.iter().find_map(|candidate| {
            span.captures(candidate).map(|captures| {
                (
                    captures[1].to_string(),
                    captures[2].parse::<u64>().ok(),
                    captures[3].parse::<u64>().ok(),
                )
            })
        });
        let rule = following.iter().find_map(|candidate| {
            lint.captures(candidate)
                .map(|captures| captures[1].to_string())
        });
        let (file, row, col) = location
            .map(|(file, row, col)| (Some(file), row, col))
            .unwrap_or((None, None, None));
        diagnostics.push(make_diagnostic(
            severity,
            file,
            row,
            col,
            message,
            rule,
            command,
            captured_at,
        ));
    }
    diagnostics
}

fn parse_typescript(text: &str, command: &str, captured_at: &str) -> Vec<Diagnostic> {
    let pattern =
        Regex::new(r"^(.+)\((\d+),(\d+)\):\s*(warning|error)\s+(TS\d+):\s*(.+)$").unwrap();
    text.lines()
        .filter_map(|line| pattern.captures(line.trim()))
        .map(|captures| {
            make_diagnostic(
                &captures[4],
                Some(captures[1].to_string()),
                captures[2].parse().ok(),
                captures[3].parse().ok(),
                &captures[6],
                Some(captures[5].to_string()),
                command,
                captured_at,
            )
        })
        .collect()
}

fn parse_cpp(text: &str, command: &str, captured_at: &str) -> Vec<Diagnostic> {
    let pattern =
        Regex::new(r"^(.+):(\d+):(\d+):\s*(warning|error):\s*(.*?)(?:\s+\[(-W[^\]]+)\])?$")
            .unwrap();
    text.lines()
        .filter_map(|line| pattern.captures(line.trim()))
        .map(|captures| {
            make_diagnostic(
                &captures[4],
                Some(captures[1].to_string()),
                captures[2].parse().ok(),
                captures[3].parse().ok(),
                captures
                    .get(5)
                    .map(|value| value.as_str())
                    .unwrap_or_default(),
                captures.get(6).map(|value| value.as_str().to_string()),
                command,
                captured_at,
            )
        })
        .collect()
}

fn parse_python(text: &str, command: &str, captured_at: &str) -> Vec<Diagnostic> {
    let pattern =
        Regex::new(r"^\s*(.+):(\d+):\s*([A-Za-z_][A-Za-z0-9_]*(?:Warning)):\s*(.+)$").unwrap();
    text.lines()
        .filter_map(|line| pattern.captures(line))
        .map(|captures| {
            make_diagnostic(
                "warning",
                Some(captures[1].trim().to_string()),
                captures[2].parse().ok(),
                None,
                &captures[4],
                Some(captures[3].to_string()),
                command,
                captured_at,
            )
        })
        .collect()
}

fn parse_package(text: &str, command: &str, captured_at: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        let marker = lower
            .find("npm warn ")
            .map(|index| index + "npm warn ".len())
            .or_else(|| lower.find("warn ").map(|index| index + "warn ".len()));
        let Some(marker) = marker else {
            continue;
        };
        let body = line[marker..].trim();
        let mut pieces = body.splitn(2, char::is_whitespace);
        let rule = pieces.next().unwrap_or("warning");
        let message = pieces.next().unwrap_or(rule).trim();
        diagnostics.push(make_diagnostic(
            "warning",
            None,
            None,
            None,
            message,
            Some(rule.to_ascii_lowercase()),
            command,
            captured_at,
        ));
    }
    diagnostics
}

fn parse_dotnet(text: &str, command: &str, captured_at: &str) -> Vec<Diagnostic> {
    let pattern = Regex::new(
        r"^(.+)\((\d+),(\d+)\):\s*(warning|error)\s+([A-Za-z]+\d+):\s*(.+?)(?:\s+\[.*\])?$",
    )
    .unwrap();
    text.lines()
        .filter_map(|line| pattern.captures(line.trim()))
        .map(|captures| {
            make_diagnostic(
                &captures[4],
                Some(captures[1].to_string()),
                captures[2].parse().ok(),
                captures[3].parse().ok(),
                &captures[6],
                Some(captures[5].to_string()),
                command,
                captured_at,
            )
        })
        .collect()
}

fn parse_colon_location(location: &str) -> Option<(String, u64, u64)> {
    let pattern = Regex::new(r"^(.+):(\d+):(\d+)$").unwrap();
    let captures = pattern.captures(location.trim())?;
    Some((
        captures[1].to_string(),
        captures[2].parse().ok()?,
        captures[3].parse().ok()?,
    ))
}

fn is_diagnostic_severity(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "info" | "warning" | "error"
    )
}

#[allow(clippy::too_many_arguments)]
fn make_diagnostic(
    severity: &str,
    file: Option<String>,
    line: Option<u64>,
    col: Option<u64>,
    message: &str,
    rule: Option<String>,
    source_command: &str,
    captured_at: &str,
) -> Diagnostic {
    let message = message.trim().to_string();
    let normalized = message
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join(" ");
    let fingerprint_material = format!(
        "{}|{}|{}|{}",
        file.as_deref().unwrap_or_default().replace('\\', "/"),
        line.unwrap_or_default(),
        rule.as_deref().unwrap_or_default().to_ascii_lowercase(),
        normalized
    );
    Diagnostic {
        severity: severity.trim().to_ascii_lowercase(),
        file,
        line,
        col,
        message,
        rule,
        source_command: source_command.to_string(),
        captured_at: captured_at.to_string(),
        fingerprint: format!(
            "fnv1a64:{}",
            crate::utility::hashing::fnv1a64_hex(&fingerprint_material)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::token_meter::TokenMeter;
    use std::path::PathBuf;

    fn ast(program: &str, arguments: &[&str]) -> CommandAst {
        CommandAst::new(
            program.to_string(),
            arguments.iter().map(|value| value.to_string()).collect(),
            PathBuf::from("workspace"),
        )
    }

    fn parsed(program: &str, arguments: &[&str], output: &str) -> Diagnostic {
        let diagnostics = parse_diagnostics(
            &ast(program, arguments),
            output.as_bytes(),
            &[],
            &format!("{program} {}", arguments.join(" ")),
            "2026-09-09T00:00:00Z",
        );
        assert_eq!(diagnostics.len(), 1, "output: {output:?}");
        diagnostics.into_iter().next().unwrap()
    }

    #[test]
    fn parses_flutter_bullet_diagnostic() {
        let diagnostic = parsed(
            "flutter",
            &["analyze"],
            "info • Avoid print calls in production code • lib/main.dart:7:3 • avoid_print\n",
        );
        assert_eq!(diagnostic.severity, "info");
        assert_eq!(diagnostic.file.as_deref(), Some("lib/main.dart"));
        assert_eq!(diagnostic.line, Some(7));
        assert_eq!(diagnostic.col, Some(3));
        assert_eq!(diagnostic.rule.as_deref(), Some("avoid_print"));
    }

    #[test]
    fn parses_dart_machine_diagnostic() {
        let diagnostic = parsed(
            "dart",
            &["analyze", "--format=machine"],
            "INFO|LINT|AVOID_PRINT|lib/main.dart|28|3|5|Don't invoke 'print' in production code.\n",
        );
        assert_eq!(diagnostic.severity, "info");
        assert_eq!(diagnostic.line, Some(28));
        assert_eq!(diagnostic.rule.as_deref(), Some("AVOID_PRINT"));
    }

    #[test]
    fn parses_rust_warning_with_following_span() {
        let diagnostic = parsed(
            "cargo",
            &["clippy"],
            "warning: unused variable: `answer`\n --> src/lib.rs:4:9\n  |\n4 | let answer = 42;\n  |     ^^^^^^ help: prefix it with an underscore\n  |\n  = note: `#[warn(unused_variables)]` on by default\n",
        );
        assert_eq!(diagnostic.severity, "warning");
        assert_eq!(diagnostic.file.as_deref(), Some("src/lib.rs"));
        assert_eq!(diagnostic.line, Some(4));
        assert_eq!(diagnostic.rule.as_deref(), Some("unused_variables"));
    }

    #[test]
    fn parses_rust_json_compiler_message() {
        let diagnostic = parsed(
            "cargo",
            &["check", "--message-format=json"],
            r#"{"reason":"compiler-message","message":{"message":"unused import","code":{"code":"unused_imports"},"level":"warning","spans":[{"file_name":"src/lib.rs","line_start":2,"column_start":5,"is_primary":true}]}}"#,
        );
        assert_eq!(diagnostic.file.as_deref(), Some("src/lib.rs"));
        assert_eq!(diagnostic.rule.as_deref(), Some("unused_imports"));
    }

    #[test]
    fn parses_typescript_warning() {
        let diagnostic = parsed(
            "tsc",
            &["--noEmit", "--pretty", "false"],
            "src/app.ts(12,7): warning TS6133: 'unused' is declared but its value is never read.\n",
        );
        assert_eq!(diagnostic.file.as_deref(), Some("src/app.ts"));
        assert_eq!(diagnostic.rule.as_deref(), Some("TS6133"));
    }

    #[test]
    fn parses_gcc_and_clang_warning() {
        let diagnostic = parsed(
            "clang",
            &["-Wall", "main.c"],
            "main.c:3:11: warning: unused variable 'value' [-Wunused-variable]\n",
        );
        assert_eq!(diagnostic.file.as_deref(), Some("main.c"));
        assert_eq!(diagnostic.rule.as_deref(), Some("-Wunused-variable"));
    }

    #[test]
    fn parses_pytest_warning_summary() {
        let diagnostic = parsed(
            "pytest",
            &["-q"],
            "================ warnings summary ================\nmy_test.py::test_one\n  C:/work/my_test.py:5: DeprecationWarning: api v1 is deprecated\n    old_api()\n-- Docs: https://docs.pytest.org/\n1 passed, 1 warning in 0.12s\n",
        );
        assert_eq!(diagnostic.file.as_deref(), Some("C:/work/my_test.py"));
        assert_eq!(diagnostic.line, Some(5));
        assert_eq!(diagnostic.rule.as_deref(), Some("DeprecationWarning"));
    }

    #[test]
    fn parses_npm_and_pnpm_deprecations() {
        let npm = parsed(
            "npm",
            &["install"],
            "npm WARN deprecated inflight@1.0.6: This module is not supported.\n",
        );
        let pnpm = parsed(
            "pnpm",
            &["install"],
            "packages/app | WARN deprecated left-pad@1.3.0\n",
        );
        assert_eq!(npm.rule.as_deref(), Some("deprecated"));
        assert_eq!(pnpm.rule.as_deref(), Some("deprecated"));
    }

    #[test]
    fn fingerprints_deduplicate_normalized_messages() {
        let diagnostics = parse_diagnostics(
            &ast("tsc", &["--noEmit"]),
            b"src/app.ts(12,7): warning TS6133: unused   value\nsrc/app.ts(12,7): warning TS6133: UNUSED value\n",
            &[],
            "tsc --noEmit",
            "2026-09-09T00:00:00Z",
        );
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn first_scan_is_baseline_then_new_warning_opens_and_clean_rerun_resolves() {
        let root = std::env::temp_dir().join(format!(
            "keel-warning-lifecycle-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let home = root.join("home");
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let command = ast("dart", &["analyze", "--format=machine"]);
        let baseline = reconcile_capture(
            &home,
            &workspace,
            &command,
            &[],
            &[],
            0,
            "dart analyze --format=machine",
            "2026-09-09T00:00:00Z",
            "raw-1",
        )
        .unwrap();
        assert_eq!(baseline, WarningSummary { open: 0, new: 0 });

        let warning = b"INFO|LINT|AVOID_PRINT|lib/main.dart|7|3|5|Avoid print.\n";
        let opened = reconcile_capture(
            &home,
            &workspace,
            &command,
            warning,
            &[],
            0,
            "dart analyze --format=machine",
            "2026-09-09T00:01:00Z",
            "raw-2",
        )
        .unwrap();
        assert_eq!(opened, WarningSummary { open: 1, new: 1 });

        let resolved = reconcile_capture(
            &home,
            &workspace,
            &command,
            &[],
            &[],
            0,
            "dart analyze --format=machine",
            "2026-09-09T00:02:00Z",
            "raw-3",
        )
        .unwrap();
        assert_eq!(resolved, WarningSummary { open: 0, new: 0 });
        let ledger = std::fs::read_to_string(
            home.join("memories")
                .join("workspaces")
                .join(crate::utility::system_map::workspace_key(
                    &workspace.to_string_lossy(),
                ))
                .join("warnings")
                .join("ledger.jsonl"),
        )
        .unwrap();
        assert!(ledger.contains("\"status\":\"open\""));
        assert!(ledger.contains("\"status\":\"resolved\""));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn pointer_stays_below_thirty_tokens() {
        let pointer = warning_pointer(&WarningSummary { open: 12, new: 3 }).unwrap();
        assert_eq!(pointer, "warnings: 12 open (3 new) — run `keel warn list`");
        assert!(TokenMeter::count_text(&pointer) < 30);
    }

    #[test]
    fn waiver_requires_reason_and_reopens_after_expiry() {
        let root = std::env::temp_dir().join(format!(
            "keel-warning-waiver-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let home = root.join("home");
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let command = ast("dart", &["analyze", "--format=machine"]);
        reconcile_capture(
            &home,
            &workspace,
            &command,
            &[],
            &[],
            0,
            "dart analyze --format=machine",
            "2026-09-09T00:00:00Z",
            "raw-1",
        )
        .unwrap();
        reconcile_capture(
            &home,
            &workspace,
            &command,
            b"INFO|LINT|AVOID_PRINT|lib/main.dart|7|3|5|Avoid print.\n",
            &[],
            0,
            "dart analyze --format=machine",
            "2026-09-09T00:01:00Z",
            "raw-2",
        )
        .unwrap();
        let fingerprint = current_warnings(&home, &workspace, "2026-09-09T00:02:00Z")
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
            .diagnostic
            .fingerprint;
        assert!(waive_warning(
            &home,
            &workspace,
            &fingerprint,
            "too short",
            "30d",
            "2026-09-09T00:02:00Z"
        )
        .is_err());
        waive_warning(
            &home,
            &workspace,
            &fingerprint,
            "Accepted until dependency migration",
            "30d",
            "2026-09-09T00:02:00Z",
        )
        .unwrap();
        let waived = current_warnings(&home, &workspace, "2026-10-08T00:02:00Z").unwrap();
        assert_eq!(waived[0].status, WarningStatus::Waived);
        assert_eq!(
            waived[0].waiver_reason.as_deref(),
            Some("Accepted until dependency migration")
        );
        reconcile_capture(
            &home,
            &workspace,
            &command,
            &[],
            &[],
            0,
            "dart analyze --format=machine",
            "2026-10-08T00:03:00Z",
            "raw-3",
        )
        .unwrap();
        let still_waived = current_warnings(&home, &workspace, "2026-10-08T00:04:00Z").unwrap();
        assert_eq!(still_waived[0].status, WarningStatus::Waived);
        let expired = current_warnings(&home, &workspace, "2026-10-10T00:02:00Z").unwrap();
        assert_eq!(expired[0].status, WarningStatus::Open);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn gate_lists_baseline_without_blocking_and_blocks_only_open() {
        let root = std::env::temp_dir().join(format!(
            "keel-warning-gate-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let home = root.join("home");
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let command = ast("dart", &["analyze", "--format=machine"]);
        reconcile_capture(
            &home,
            &workspace,
            &command,
            b"INFO|LINT|AVOID_PRINT|lib/old.dart|4|3|5|Old warning.\n",
            &[],
            0,
            "dart analyze --format=machine",
            "2026-09-09T00:00:00Z",
            "raw-1",
        )
        .unwrap();
        let baseline = warning_gate(&home, &workspace, "2026-09-09T00:01:00Z").unwrap();
        assert!(!baseline.blocking);
        assert_eq!(baseline.baseline, 1);
        reconcile_capture(
            &home,
            &workspace,
            &command,
            b"INFO|LINT|AVOID_PRINT|lib/old.dart|4|3|5|Old warning.\nINFO|LINT|DEAD_CODE|lib/new.dart|8|2|4|Dead code.\n",
            &[],
            0,
            "dart analyze --format=machine",
            "2026-09-09T00:02:00Z",
            "raw-2",
        )
        .unwrap();
        let opened = warning_gate(&home, &workspace, "2026-09-09T00:03:00Z").unwrap();
        assert!(opened.blocking);
        assert_eq!(opened.open, 1);
        assert_eq!(opened.baseline, 1);
        assert!(opened.details.contains("baseline=1"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn open_warning_adds_lint_warnings_subtask_without_replacing_existing() {
        let root = std::env::temp_dir().join(format!(
            "keel-warning-ticket-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let home = root.join("home");
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let plan = home
            .join("memories")
            .join("workspaces")
            .join(crate::utility::system_map::workspace_key(
                &workspace.to_string_lossy(),
            ))
            .join("plans")
            .join("plan-test");
        std::fs::create_dir_all(&plan).unwrap();
        std::fs::write(
            plan.join("task-001.json"),
            r#"{
  "schema_version": 1,
  "artifact": "task_ticket",
  "plan_id": "plan-test",
  "id": "TASK-001",
  "title": "Keep analyzer clean",
  "requirement_refs": ["REQ-001"],
  "acceptance_refs": ["AC-001"],
  "status": "planned",
  "layers": {
    "lint_warnings": [
      {
        "id": "TASK-001-LINT-WARNINGS-001",
        "description": "Produce lint_warnings evidence for TASK-001.",
        "requirement_refs": ["REQ-001"],
        "acceptance_refs": ["AC-001"],
        "status": "open",
        "expected_evidence_type": "lint_diagnostic",
        "evidence_ref": null,
        "reason": null,
        "owner_role": "verifier",
        "verification_timestamp": null,
        "derived": true
      }
    ]
  }
}
"#,
        )
        .unwrap();
        std::fs::write(
            plan.join("rtm.json"),
            r#"{
  "schemaVersion": 1,
  "artifact": "rtm",
  "planId": "plan-test",
  "status": "complete",
  "chain": "User request -> Requirement -> Acceptance criterion -> Task -> Checklist subtask -> Evidence",
  "entries": [
    {
      "requirementId": "REQ-001",
      "taskIds": ["TASK-001"],
      "checklistSubtaskIds": ["TASK-001-LINT-WARNINGS-001"]
    }
  ],
  "traces": [
    {
      "userRequestRef": "request://submitted",
      "requirementId": "REQ-001",
      "acceptanceCriterionId": "AC-001",
      "taskId": "TASK-001",
      "subtaskId": "TASK-001-LINT-WARNINGS-001",
      "expectedEvidenceType": "lint_diagnostic",
      "evidenceRef": null
    }
  ]
}
"#,
        )
        .unwrap();

        let command = ast("dart", &["analyze", "--format=machine"]);
        reconcile_capture(
            &home,
            &workspace,
            &command,
            &[],
            &[],
            0,
            "dart analyze --format=machine",
            "2026-09-09T00:00:00Z",
            "raw-1",
        )
        .unwrap();
        reconcile_capture(
            &home,
            &workspace,
            &command,
            b"INFO|LINT|AVOID_PRINT|lib/main.dart|7|3|5|Avoid print.\n",
            &[],
            0,
            "dart analyze --format=machine",
            "2026-09-09T00:01:00Z",
            "raw-2",
        )
        .unwrap();

        let ticket: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(plan.join("task-001.json")).unwrap())
                .unwrap();
        let subtasks = ticket["layers"]["lint_warnings"].as_array().unwrap();
        assert_eq!(subtasks.len(), 2, "ticket: {ticket}");
        assert_eq!(
            subtasks[0]["id"].as_str(),
            Some("TASK-001-LINT-WARNINGS-001")
        );
        let fingerprint = current_warnings(&home, &workspace, "2026-09-09T00:01:00Z")
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
            .diagnostic
            .fingerprint;
        assert!(
            subtasks.iter().any(|subtask| {
                subtask["id"].as_str()
                    == Some(format!("TASK-001-LINT-WARNINGS-{fingerprint}").as_str())
                    || subtask["description"]
                        .as_str()
                        .is_some_and(|text| text.contains(&fingerprint))
            }),
            "missing warning subtask in {subtasks:?}"
        );

        let rtm: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(plan.join("rtm.json")).unwrap()).unwrap();
        let traces = rtm["traces"].as_array().unwrap();
        assert_eq!(traces.len(), 2, "rtm: {rtm}");
        assert!(traces
            .iter()
            .any(|trace| { trace["subtaskId"].as_str() == Some("TASK-001-LINT-WARNINGS-001") }));
        let _ = std::fs::remove_dir_all(root);
    }
}
