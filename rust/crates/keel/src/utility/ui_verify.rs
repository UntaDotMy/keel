//! Purpose: UI/UX visual verification and evidence engine for Keel.
//! Caller: manager::verify::run_verify_command and review diff_gates.
//! Dependencies: std::fs, serde, serde_json, crate::args, crate::proxy::raw_store.
//! Main Functions: run_verify_ui_command, evaluate_visual_criterion, detect_visual_adapter.
//! Side Effects: Captures/evaluates visual fixtures and saves screenshots in RawStore.

use std::fs;
use std::io::Write;
use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::args::FlagSet;
use crate::proxy::raw_store::{RawNamespace, RawStore};
use crate::runtime::{
    display_path, resolve_claude_home, resolve_repository_root, safe_path_segment, write_text,
};

const MAX_FIXTURE_READ_BYTES: usize = 16 * 1024 * 1024;
const MAX_UI_REASON_COUNT: usize = 16;
const MAX_UI_REASON_CHARS: usize = 512;

const MINIMAL_PNG_BYTES: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UiCriterion {
    pub state_name: String,
    pub route_or_screen: String,
    #[serde(default)]
    pub preconditions: Vec<String>,
    #[serde(default)]
    pub fixture_data: Option<Value>,
    #[serde(default)]
    pub auth_condition: Option<String>,
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default)]
    pub expected_visible_text: Vec<String>,
    #[serde(default)]
    pub expected_interactions: Vec<String>,
    #[serde(default)]
    pub layout_responsiveness: Option<String>,
    #[serde(default)]
    pub accessibility_expectations: Option<String>,
    #[serde(default)]
    pub screenshot_required: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VisualAdapter {
    ComputerUse,
    Playwright,
    NeedsHuman,
}

impl VisualAdapter {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ComputerUse => "computer-use",
            Self::Playwright => "playwright",
            Self::NeedsHuman => "needs_human",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "computer-use" | "computer_use" | "browser" => Some(Self::ComputerUse),
            "playwright" => Some(Self::Playwright),
            "needs-human" | "needs_human" | "human" => Some(Self::NeedsHuman),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VisualVerdict {
    Pass,
    Fail,
    Unclear,
    NeedsHuman,
}

impl VisualVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Unclear => "unclear",
            Self::NeedsHuman => "needs_human",
        }
    }

    pub fn to_review_status(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Unclear | Self::NeedsHuman => "needs_human",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UiEvidenceRecord {
    pub state: String,
    pub screenshot_id: String,
    pub captured_at: String,
    pub adapter: String,
    pub verdict: String,
    pub review_status: String,
    pub reasons: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedUiVerificationManifest {
    schema_version: u32,
    artifact: String,
    task_id: String,
    plan_id: Option<String>,
    workspace_root: String,
    updated_at: String,
    status: String,
    verdicts_path: String,
    latest_screenshot_id: String,
    latest_review_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedUiVerificationVerdict {
    schema_version: u32,
    state: String,
    screenshot_id: String,
    captured_at: String,
    adapter: String,
    verdict: String,
    review_status: String,
    reasons: Vec<String>,
}

pub fn detect_visual_adapter(workspace_root: &Path, explicit: Option<&str>) -> VisualAdapter {
    if let Some(name) = explicit {
        if let Some(adapter) = VisualAdapter::from_name(name) {
            return adapter;
        }
    }
    if std::env::var("CLAUDE_COMPUTER_USE").is_ok()
        || std::env::var("KEEL_BROWSER_AVAILABLE").is_ok()
    {
        return VisualAdapter::ComputerUse;
    }
    let playwright_config = workspace_root.join("playwright.config.ts");
    let playwright_config_js = workspace_root.join("playwright.config.js");
    let package_json = workspace_root.join("package.json");
    let has_playwright_package = if package_json.is_file() {
        fs::read_to_string(&package_json)
            .map(|content| content.contains("playwright"))
            .unwrap_or(false)
    } else {
        false
    };
    if playwright_config.is_file()
        || playwright_config_js.is_file()
        || has_playwright_package
        || std::env::var("PLAYWRIGHT_AVAILABLE").is_ok()
    {
        return VisualAdapter::Playwright;
    }
    VisualAdapter::NeedsHuman
}

pub fn evaluate_visual_criterion(
    criterion: &UiCriterion,
    fixture_value: Option<&Value>,
    adapter: VisualAdapter,
) -> (VisualVerdict, Vec<String>) {
    if adapter == VisualAdapter::NeedsHuman {
        return (
            VisualVerdict::NeedsHuman,
            vec![
                "No automated visual adapter available (neither computer-use nor playwright detected); requires human visual inspection"
                    .to_string(),
            ],
        );
    }
    let Some(fixture) = fixture_value else {
        return (
            VisualVerdict::Unclear,
            vec![
                "No screen fixture or navigation context available to verify criteria".to_string(),
            ],
        );
    };

    if let Some(explicit_verdict) = fixture.get("verdict").and_then(Value::as_str) {
        let reasons = fixture
            .get("reasons")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_else(|| {
                vec![format!(
                    "Fixture declared explicit verdict {explicit_verdict}"
                )]
            });

        match explicit_verdict {
            "pass" => return (VisualVerdict::Pass, reasons),
            "fail" => return (VisualVerdict::Fail, reasons),
            "unclear" => return (VisualVerdict::Unclear, reasons),
            "needs_human" => return (VisualVerdict::NeedsHuman, reasons),
            _ => {}
        }
    }

    if fixture.get("unclear").and_then(Value::as_bool) == Some(true) {
        let reason = fixture
            .get("unclear_reason")
            .and_then(Value::as_str)
            .unwrap_or("Visual evaluation is unclear");
        return (
            VisualVerdict::Unclear,
            vec![format!("Visual inspection required: {reason}")],
        );
    }

    let mut reasons = Vec::new();
    let mut missing_elements = Vec::new();

    let fixture_text = extract_fixture_text(fixture);

    for expected in &criterion.expected_visible_text {
        if !fixture_text.contains(expected.trim()) {
            missing_elements.push(expected.clone());
        }
    }

    if !missing_elements.is_empty() {
        for missing in &missing_elements {
            reasons.push(format!("Missing expected visible text: \"{missing}\""));
        }
        return (VisualVerdict::Fail, reasons);
    }

    reasons.push(format!(
        "Verified {} expected visible element(s) on screen \"{}\"",
        criterion.expected_visible_text.len(),
        criterion.route_or_screen
    ));

    (VisualVerdict::Pass, reasons)
}

fn extract_fixture_text(fixture: &Value) -> String {
    let mut buffer = String::new();
    if let Some(text) = fixture.get("text").and_then(Value::as_str) {
        buffer.push_str(text);
        buffer.push(' ');
    }
    if let Some(screen_text) = fixture.get("screen_text") {
        if let Some(s) = screen_text.as_str() {
            buffer.push_str(s);
            buffer.push(' ');
        } else if let Some(arr) = screen_text.as_array() {
            for item in arr {
                if let Some(s) = item.as_str() {
                    buffer.push_str(s);
                    buffer.push(' ');
                }
            }
        }
    }
    if let Some(visible_text) = fixture.get("visible_text") {
        if let Some(s) = visible_text.as_str() {
            buffer.push_str(s);
            buffer.push(' ');
        } else if let Some(arr) = visible_text.as_array() {
            for item in arr {
                if let Some(s) = item.as_str() {
                    buffer.push_str(s);
                    buffer.push(' ');
                }
            }
        }
    }
    if let Some(html) = fixture.get("html").and_then(Value::as_str) {
        buffer.push_str(html);
        buffer.push(' ');
    }
    if let Some(elements) = fixture.get("elements").and_then(Value::as_array) {
        for el in elements {
            if let Some(s) = el.as_str() {
                buffer.push_str(s);
                buffer.push(' ');
            } else if let Some(text) = el.get("text").and_then(Value::as_str) {
                buffer.push_str(text);
                buffer.push(' ');
            }
        }
    }
    buffer
}

pub fn run_verify_ui_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("verify ui");
    flag_set.string_flag("task", "");
    flag_set.string_flag("fixture", "");
    flag_set.string_flag("plan", "");
    flag_set.string_flag("adapter", "");
    flag_set.string_flag("workspace-root", "");
    flag_set.string_flag("claude-home", "");
    flag_set.bool_flag("json", false);

    if let Err(error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "verify ui: {}", error.message);
        return 1;
    }

    let repository_root = match resolve_repository_root(flag_set.string_value("workspace-root")) {
        Ok(root) => root,
        Err(error) => {
            let _ = writeln!(standard_error, "verify ui: {error}");
            return 1;
        }
    };

    let claude_home_flag = flag_set.string_value("claude-home");
    let explicit_adapter = flag_set.string_value("adapter");
    let adapter = detect_visual_adapter(
        &repository_root,
        if explicit_adapter.is_empty() {
            None
        } else {
            Some(explicit_adapter)
        },
    );

    let (criterion, fixture_val, screenshot_data) = match load_criterion_and_fixture(
        &repository_root,
        flag_set.string_value("fixture"),
        flag_set.string_value("task"),
        flag_set.string_value("plan"),
        claude_home_flag,
    ) {
        Ok(result) => result,
        Err(error) => {
            let _ = writeln!(standard_error, "verify ui: {error}");
            return 1;
        }
    };

    let (verdict, reasons) = evaluate_visual_criterion(&criterion, fixture_val.as_ref(), adapter);
    let reasons = bound_ui_reasons(reasons);

    let raw_store_root = resolve_claude_home(claude_home_flag)
        .map(|home| home.join("raw-output"))
        .unwrap_or_else(|_| repository_root.join(".keel/raw-output"));
    let session_id = ["CLAUDE_CODE_SESSION_ID", "CODEX_THREAD_ID"]
        .iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "default".to_string());
    let store = RawStore::with_namespace(
        raw_store_root,
        RawNamespace {
            workspace_id: repository_root.to_string_lossy().to_string(),
            session_id,
        },
    );

    let raw_id = RawStore::generate_id();
    let command_line = format!("keel verify ui --state {}", criterion.state_name);
    let captured_at = Utc::now().to_rfc3339();

    let exit_code = if verdict == VisualVerdict::Fail { 1 } else { 0 };

    let png_bytes = screenshot_data.as_deref().unwrap_or(MINIMAL_PNG_BYTES);

    let stdout_summary = format!(
        "state: {}\nadapter: {}\nverdict: {}\nreasons: {}\n",
        criterion.state_name,
        adapter.as_str(),
        verdict.as_str(),
        reasons.join("; ")
    );

    if let Err(error) = store.save_screenshot(
        &raw_id,
        &command_line,
        stdout_summary.as_bytes(),
        &[],
        png_bytes,
        exit_code,
    ) {
        let _ = writeln!(
            standard_error,
            "verify ui: failed to save screenshot in RawStore: {error}"
        );
        return 1;
    }

    let evidence_record = UiEvidenceRecord {
        state: criterion.state_name.clone(),
        screenshot_id: raw_id.clone(),
        captured_at,
        adapter: adapter.as_str().to_string(),
        verdict: verdict.as_str().to_string(),
        review_status: verdict.to_review_status().to_string(),
        reasons: reasons.clone(),
        metadata: fixture_val,
    };

    if let Err(error) = persist_ui_verification_artifacts(
        &resolve_claude_home(claude_home_flag).unwrap_or_else(|_| repository_root.join(".keel")),
        &repository_root,
        flag_set.string_value("task"),
        flag_set.string_value("plan"),
        &evidence_record,
    ) {
        let _ = writeln!(
            standard_error,
            "verify ui: failed to persist verification manifest: {error}"
        );
        return 1;
    }

    let context_projection = match project_ui_evidence(&evidence_record, &repository_root) {
        Ok(projection) => projection,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "verify ui: context firewall rejected evidence: {error}"
            );
            return 1;
        }
    };

    if flag_set.bool_value("json") {
        let mut output = match serde_json::to_value(&evidence_record) {
            Ok(value) => value,
            Err(error) => {
                let _ = writeln!(standard_error, "verify ui: serialize evidence: {error}");
                return 1;
            }
        };
        // Keep the public field for compatibility, but replace large fixture
        // values with a bounded recovery pointer outside model context.
        output["metadata"] = json!({
            "omitted": evidence_record.metadata.is_some(),
            "rawArtifactId": evidence_record.screenshot_id,
        });
        output["summary"] = json!(context_projection.summary);
        output["context"] =
            serde_json::to_value(context_projection.metadata()).unwrap_or_else(|_| json!({}));
        if let Ok(json_str) = serde_json::to_string_pretty(&output) {
            let _ = writeln!(standard_output, "{json_str}");
        }
    } else {
        let _ = writeln!(standard_output, "{}", context_projection.summary);
    }

    if exit_code != 0 {
        1
    } else {
        0
    }
}

fn bound_ui_reasons(reasons: Vec<String>) -> Vec<String> {
    let omitted = reasons.len().saturating_sub(MAX_UI_REASON_COUNT);
    let mut bounded = reasons
        .into_iter()
        .take(MAX_UI_REASON_COUNT)
        .map(|reason| truncate_ui_text(&reason, MAX_UI_REASON_CHARS))
        .collect::<Vec<_>>();
    if omitted > 0 {
        bounded.push(format!(
            "{omitted} additional visual reason(s) omitted; inspect the raw screenshot artifact"
        ));
    }
    bounded
}

fn truncate_ui_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut truncated = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

fn project_ui_evidence(
    evidence: &UiEvidenceRecord,
    repository_root: &Path,
) -> Result<crate::proxy::context::ContextProjection, String> {
    let session_id = ["CLAUDE_CODE_SESSION_ID", "CODEX_THREAD_ID"]
        .iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "default".to_string());
    let summary = format!(
        "UI Verification Result:\nstate: {}\nadapter: {}\nverdict: {}\nreview_status: {}\nscreenshot_id: {}\nreasons: {}",
        evidence.state,
        evidence.adapter,
        evidence.verdict,
        evidence.review_status,
        evidence.screenshot_id,
        evidence.reasons.join("; "),
    );
    crate::proxy::context::project_scoped(
        crate::proxy::context::ContextPolicy::for_surface(
            crate::proxy::context::DEFAULT_MAX_SINGLE_RESULT_TOKENS,
        ),
        crate::proxy::context::ProjectionInput::new(
            crate::proxy::context::ContextSource::UiVerification,
            summary,
            Some(evidence.screenshot_id.clone()),
            repository_root.to_string_lossy().to_string(),
            session_id,
        )
        .with_cache_class(crate::proxy::context::CacheClass::Session),
    )
    .map_err(|error| error.to_string())
}

/// Persist a compact, schema-versioned UI verification index. Screenshots and
/// full fixture data remain in RawStore; these artifacts carry only the state,
/// verdict, reasons, and recovery id needed by review/completion gates.
fn persist_ui_verification_artifacts(
    claude_home: &Path,
    repository_root: &Path,
    task_id: &str,
    plan_id: &str,
    evidence: &UiEvidenceRecord,
) -> Result<(), String> {
    let requested = if !task_id.trim().is_empty() {
        task_id.trim()
    } else if !plan_id.trim().is_empty() {
        plan_id.trim()
    } else {
        "ad-hoc"
    };
    let task_key = safe_path_segment(requested)
        .ok_or_else(|| format!("invalid UI verification task id {requested:?}"))?;
    let workspace_key =
        crate::utility::system_map::workspace_key(&repository_root.to_string_lossy());
    let directory = claude_home
        .join("memories")
        .join("workspaces")
        .join(workspace_key)
        .join("ui-verification")
        .join(task_key);
    fs::create_dir_all(&directory)
        .map_err(|error| format!("create {}: {error}", display_path(&directory)))?;

    let manifest_path = directory.join("manifest.json");
    let verdicts_path = directory.join("verdicts.json");
    let manifest_exists = manifest_path.is_file();
    let verdicts_exists = verdicts_path.is_file();
    if verdicts_exists && !manifest_exists {
        return Err(format!(
            "{} exists without a schema manifest",
            display_path(&verdicts_path)
        ));
    }
    if manifest_exists {
        let existing = fs::read_to_string(&manifest_path)
            .map_err(|error| format!("read {}: {error}", display_path(&manifest_path)))?;
        let manifest = serde_json::from_str::<PersistedUiVerificationManifest>(&existing)
            .map_err(|error| format!("parse {}: {error}", display_path(&manifest_path)))?;
        validate_ui_manifest(&manifest, requested, repository_root)?;
    }
    let mut verdicts: Vec<PersistedUiVerificationVerdict> = if verdicts_exists {
        let existing = fs::read_to_string(&verdicts_path)
            .map_err(|error| format!("read {}: {error}", display_path(&verdicts_path)))?;
        let parsed = serde_json::from_str::<Vec<PersistedUiVerificationVerdict>>(&existing)
            .map_err(|error| format!("parse {}: {error}", display_path(&verdicts_path)))?;
        for verdict in &parsed {
            validate_ui_verdict(verdict)?;
        }
        parsed
    } else {
        Vec::new()
    };
    let verdict = PersistedUiVerificationVerdict {
        schema_version: 1,
        state: evidence.state.clone(),
        screenshot_id: evidence.screenshot_id.clone(),
        captured_at: evidence.captured_at.clone(),
        adapter: evidence.adapter.clone(),
        verdict: evidence.verdict.clone(),
        // Promote unclear at the gate boundary; retain the raw verdict for
        // diagnosis while completion records the required human review.
        review_status: if evidence.verdict == "unclear" {
            "needs_human".to_string()
        } else {
            evidence.review_status.clone()
        },
        reasons: evidence.reasons.clone(),
    };
    validate_ui_verdict(&verdict)?;
    verdicts.push(verdict);
    let manifest = PersistedUiVerificationManifest {
        schema_version: 1,
        artifact: "ui-verification".to_string(),
        task_id: requested.to_string(),
        plan_id: (!plan_id.trim().is_empty()).then(|| plan_id.trim().to_string()),
        workspace_root: display_path(repository_root),
        updated_at: evidence.captured_at.clone(),
        status: if evidence.verdict == "pass" {
            "pass".to_string()
        } else {
            "needs_human".to_string()
        },
        verdicts_path: "verdicts.json".to_string(),
        latest_screenshot_id: evidence.screenshot_id.clone(),
        latest_review_status: if evidence.verdict == "unclear" {
            "needs_human".to_string()
        } else {
            evidence.review_status.clone()
        },
    };
    validate_ui_manifest(&manifest, requested, repository_root)?;
    let verdicts_text = serde_json::to_string_pretty(&verdicts)
        .map_err(|error| format!("serialize verdicts: {error}"))?;
    write_text(&verdicts_path, &format!("{verdicts_text}\n"))?;
    let manifest_text = serde_json::to_string_pretty(&manifest)
        .map_err(|error| format!("serialize manifest: {error}"))?;
    write_text(&manifest_path, &format!("{manifest_text}\n"))?;
    Ok(())
}

fn validate_ui_manifest(
    manifest: &PersistedUiVerificationManifest,
    requested_task: &str,
    repository_root: &Path,
) -> Result<(), String> {
    if manifest.schema_version != 1 {
        return Err(format!(
            "unsupported UI verification manifest schema version {}",
            manifest.schema_version
        ));
    }
    if manifest.artifact != "ui-verification" {
        return Err("UI verification manifest has an invalid artifact kind".to_string());
    }
    if manifest.task_id != requested_task {
        return Err("UI verification manifest task id does not match its directory".to_string());
    }
    if manifest.workspace_root != display_path(repository_root) {
        return Err(
            "UI verification manifest workspace does not match the current workspace".to_string(),
        );
    }
    if manifest.verdicts_path != "verdicts.json" {
        return Err("UI verification manifest points outside its verdicts file".to_string());
    }
    if !matches!(manifest.status.as_str(), "pass" | "needs_human")
        || !matches!(
            manifest.latest_review_status.as_str(),
            "pass" | "fail" | "needs_human"
        )
        || safe_path_segment(&manifest.latest_screenshot_id).is_none()
    {
        return Err(
            "UI verification manifest contains an invalid status or screenshot id".to_string(),
        );
    }
    Ok(())
}

fn validate_ui_verdict(verdict: &PersistedUiVerificationVerdict) -> Result<(), String> {
    if verdict.schema_version != 1 {
        return Err(format!(
            "unsupported UI verification verdict schema version {}",
            verdict.schema_version
        ));
    }
    if verdict.state.trim().is_empty()
        || safe_path_segment(&verdict.screenshot_id).is_none()
        || !matches!(
            verdict.adapter.as_str(),
            "computer-use" | "playwright" | "needs_human"
        )
        || !matches!(
            verdict.verdict.as_str(),
            "pass" | "fail" | "unclear" | "needs_human"
        )
        || !matches!(
            verdict.review_status.as_str(),
            "pass" | "fail" | "needs_human"
        )
    {
        return Err("UI verification verdict contains an invalid identity or status".to_string());
    }
    Ok(())
}

type LoadedFixture = (UiCriterion, Option<Value>, Option<Vec<u8>>);

fn load_criterion_and_fixture(
    repository_root: &Path,
    fixture_path_str: &str,
    task_id: &str,
    plan_id: &str,
    claude_home_flag: &str,
) -> Result<LoadedFixture, String> {
    if !fixture_path_str.trim().is_empty() {
        let fixture_path = resolve_fixture_path(repository_root, fixture_path_str.trim())?;
        if !fixture_path.is_file() {
            return Err(format!(
                "fixture path not found: {}",
                display_path(&fixture_path)
            ));
        }
        let content = fs::read_to_string(&fixture_path)
            .map_err(|e| format!("read fixture {}: {e}", display_path(&fixture_path)))?;
        if content.len() > MAX_FIXTURE_READ_BYTES {
            return Err("fixture exceeds maximum size bound (16 MB)".to_string());
        }
        let parsed: Value = serde_json::from_str(&content)
            .map_err(|e| format!("parse fixture JSON {}: {e}", display_path(&fixture_path)))?;

        let criterion: UiCriterion = if let Some(criterion_val) = parsed.get("criterion") {
            serde_json::from_value(criterion_val.clone())
                .map_err(|e| format!("parse criterion in fixture: {e}"))?
        } else {
            serde_json::from_value(parsed.clone()).unwrap_or_else(|_| UiCriterion {
                state_name: "fixture-state".to_string(),
                route_or_screen: "/".to_string(),
                preconditions: Vec::new(),
                fixture_data: Some(parsed.clone()),
                auth_condition: None,
                actions: Vec::new(),
                expected_visible_text: Vec::new(),
                expected_interactions: Vec::new(),
                layout_responsiveness: None,
                accessibility_expectations: None,
                screenshot_required: true,
            })
        };

        let screenshot_data = parsed
            .get("screenshot_base64")
            .and_then(Value::as_str)
            .and_then(decode_base64);

        return Ok((criterion, Some(parsed), screenshot_data));
    }

    if !task_id.trim().is_empty() && !plan_id.trim().is_empty() {
        let workspace_key =
            crate::utility::system_map::workspace_key(&repository_root.to_string_lossy());
        let plan_key = safe_path_segment(plan_id.trim())
            .ok_or_else(|| format!("invalid plan id {:?}", plan_id.trim()))?;
        let task_key = safe_path_segment(task_id.trim())
            .ok_or_else(|| format!("invalid task id {:?}", task_id.trim()))?;
        let plan_dir = resolve_claude_home(claude_home_flag)
            .map_err(|e| e.to_string())?
            .join("memories/workspaces")
            .join(workspace_key)
            .join("plans")
            .join(plan_key);

        let ticket_file = plan_dir.join(format!("{}.json", task_key.to_ascii_lowercase()));
        if ticket_file.is_file() {
            let content = fs::read_to_string(&ticket_file)
                .map_err(|e| format!("read ticket {}: {e}", display_path(&ticket_file)))?;
            let ticket: Value = serde_json::from_str(&content)
                .map_err(|e| format!("parse ticket {}: {e}", display_path(&ticket_file)))?;

            let criterion = UiCriterion {
                state_name: format!("{task_id}-verification"),
                route_or_screen: "/".to_string(),
                preconditions: Vec::new(),
                fixture_data: None,
                auth_condition: None,
                actions: Vec::new(),
                expected_visible_text: Vec::new(),
                expected_interactions: Vec::new(),
                layout_responsiveness: None,
                accessibility_expectations: None,
                screenshot_required: true,
            };
            return Ok((criterion, Some(ticket), None));
        }
    }

    Ok((
        UiCriterion {
            state_name: "default".to_string(),
            route_or_screen: "/".to_string(),
            preconditions: Vec::new(),
            fixture_data: None,
            auth_condition: None,
            actions: Vec::new(),
            expected_visible_text: Vec::new(),
            expected_interactions: Vec::new(),
            layout_responsiveness: None,
            accessibility_expectations: None,
            screenshot_required: true,
        },
        None,
        None,
    ))
}

fn resolve_fixture_path(
    repository_root: &Path,
    fixture: &str,
) -> Result<std::path::PathBuf, String> {
    let candidate = Path::new(fixture);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        repository_root.join(candidate)
    };
    let canonical_root = repository_root.canonicalize().map_err(|error| {
        format!(
            "canonicalize workspace {}: {error}",
            display_path(repository_root)
        )
    })?;
    let canonical_fixture = joined.canonicalize().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!("fixture path not found: {}", display_path(&joined))
        } else {
            format!("canonicalize fixture {}: {error}", display_path(&joined))
        }
    })?;
    if !canonical_fixture.starts_with(&canonical_root) {
        return Err(format!(
            "fixture path escapes workspace boundary: {}",
            display_path(&canonical_fixture)
        ));
    }
    Ok(canonical_fixture)
}

fn decode_base64(encoded: &str) -> Option<Vec<u8>> {
    let clean: String = encoded.chars().filter(|c| !c.is_whitespace()).collect();
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut table = [255u8; 256];
    for (i, &b) in alphabet.iter().enumerate() {
        table[b as usize] = i as u8;
    }
    let mut output = Vec::with_capacity(clean.len() * 3 / 4);
    let bytes = clean.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' {
            break;
        }
        let b0 = table[bytes[i] as usize];
        let b1 = if i + 1 < bytes.len() && bytes[i + 1] != b'=' {
            table[bytes[i + 1] as usize]
        } else {
            0
        };
        let b2 = if i + 2 < bytes.len() && bytes[i + 2] != b'=' {
            table[bytes[i + 2] as usize]
        } else {
            0
        };
        let b3 = if i + 3 < bytes.len() && bytes[i + 3] != b'=' {
            table[bytes[i + 3] as usize]
        } else {
            0
        };
        if b0 == 255 || b1 == 255 {
            break;
        }
        output.push((b0 << 2) | (b1 >> 4));
        if i + 2 < bytes.len() && bytes[i + 2] != b'=' {
            output.push(((b1 & 0x0F) << 4) | (b2 >> 2));
        }
        if i + 3 < bytes.len() && bytes[i + 3] != b'=' {
            output.push(((b2 & 0x03) << 6) | b3);
        }
        i += 4;
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_adapter_detection_priority() {
        let temp = crate::test_support::unique_temp_dir("ui-adapter-test");
        assert_eq!(
            detect_visual_adapter(&temp, None),
            VisualAdapter::NeedsHuman
        );

        assert_eq!(
            detect_visual_adapter(&temp, Some("playwright")),
            VisualAdapter::Playwright
        );
        assert_eq!(
            detect_visual_adapter(&temp, Some("computer-use")),
            VisualAdapter::ComputerUse
        );
        assert_eq!(
            detect_visual_adapter(&temp, Some("needs_human")),
            VisualAdapter::NeedsHuman
        );

        std::fs::write(temp.join("playwright.config.ts"), "// playwright config").unwrap();
        assert_eq!(
            detect_visual_adapter(&temp, None),
            VisualAdapter::Playwright
        );
    }

    #[test]
    fn evaluate_visual_criterion_pass_fail_unclear() {
        let criterion = UiCriterion {
            state_name: "login-success".to_string(),
            route_or_screen: "/login".to_string(),
            preconditions: vec!["Valid credentials".to_string()],
            fixture_data: None,
            auth_condition: Some("anonymous".to_string()),
            actions: vec!["click #submit".to_string()],
            expected_visible_text: vec!["Welcome back".to_string(), "Dashboard".to_string()],
            expected_interactions: Vec::new(),
            layout_responsiveness: None,
            accessibility_expectations: None,
            screenshot_required: true,
        };

        let passing_fixture = serde_json::json!({
            "text": "Welcome back! Redirecting to Dashboard..."
        });
        let (verdict, reasons) = evaluate_visual_criterion(
            &criterion,
            Some(&passing_fixture),
            VisualAdapter::Playwright,
        );
        assert_eq!(verdict, VisualVerdict::Pass);
        assert!(reasons[0].contains("Verified 2 expected visible element(s)"));

        let failing_fixture = serde_json::json!({
            "text": "Invalid username or password"
        });
        let (verdict, reasons) = evaluate_visual_criterion(
            &criterion,
            Some(&failing_fixture),
            VisualAdapter::Playwright,
        );
        assert_eq!(verdict, VisualVerdict::Fail);
        assert!(reasons[0].contains("Missing expected visible text"));

        let unclear_fixture = serde_json::json!({
            "verdict": "unclear",
            "reasons": ["Element overlap detected between logo and banner"]
        });
        let (verdict, reasons) = evaluate_visual_criterion(
            &criterion,
            Some(&unclear_fixture),
            VisualAdapter::Playwright,
        );
        assert_eq!(verdict, VisualVerdict::Unclear);
        assert_eq!(verdict.to_review_status(), "needs_human");
        assert!(reasons[0].contains("Element overlap"));

        let (verdict, reasons) = evaluate_visual_criterion(
            &criterion,
            Some(&passing_fixture),
            VisualAdapter::NeedsHuman,
        );
        assert_eq!(verdict, VisualVerdict::NeedsHuman);
        assert_eq!(verdict.to_review_status(), "needs_human");
        assert!(reasons[0].contains("No automated visual adapter available"));
    }

    #[test]
    fn raw_store_screenshot_storage_and_retrieval() {
        let temp = crate::test_support::unique_temp_dir("ui-rawstore-test");
        let store = RawStore::with_root(temp.to_path_buf());
        let raw_id = RawStore::generate_id();

        let dir = store
            .save_screenshot(
                &raw_id,
                "keel verify ui --state test",
                b"pass evaluation",
                b"",
                MINIMAL_PNG_BYTES,
                0,
            )
            .expect("save screenshot");

        assert!(dir.join("screenshot.png").is_file());
        assert!(dir.join("stdout.log").is_file());
        assert!(dir.join("command.txt").is_file());
        assert!(dir.join("meta.json").is_file());

        let found = store.find_dir(&raw_id).expect("find dir");
        assert_eq!(found, dir);
    }

    #[test]
    fn ui_verification_schema_validation_rejects_unsupported_versions() {
        let root = Path::new("C:/workspace");
        let manifest = PersistedUiVerificationManifest {
            schema_version: 2,
            artifact: "ui-verification".to_string(),
            task_id: "task".to_string(),
            plan_id: None,
            workspace_root: display_path(root),
            updated_at: "2026-09-09T00:00:00Z".to_string(),
            status: "pass".to_string(),
            verdicts_path: "verdicts.json".to_string(),
            latest_screenshot_id: "20260909-000000-a1b2c3d4".to_string(),
            latest_review_status: "pass".to_string(),
        };
        assert!(validate_ui_manifest(&manifest, "task", root)
            .expect_err("unsupported manifest version")
            .contains("unsupported UI verification manifest schema version"));

        let verdict = PersistedUiVerificationVerdict {
            schema_version: 2,
            state: "state".to_string(),
            screenshot_id: "20260909-000000-a1b2c3d4".to_string(),
            captured_at: "2026-09-09T00:00:00Z".to_string(),
            adapter: "playwright".to_string(),
            verdict: "pass".to_string(),
            review_status: "pass".to_string(),
            reasons: Vec::new(),
        };
        assert!(validate_ui_verdict(&verdict)
            .expect_err("unsupported verdict version")
            .contains("unsupported UI verification verdict schema version"));
    }
}
