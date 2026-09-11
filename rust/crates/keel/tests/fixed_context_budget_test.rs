//! CLI contract for the runtime-computed `o200k_base` fixed-context budget gate.
//! Production owns canonical sources and reviewed thresholds.

use assert_cmd::Command;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const EXPECTED_SURFACES: &[&str] = &[
    "mcp.tools_list.handshake",
    "mcp.tools_list.catalog",
    "hook.session_start.bootstrap",
    "hook.user_prompt_submit.simple",
    "hook.user_prompt_submit.code_change",
    "repo.CLAUDE.md",
    "repo.AGENTS.md",
    "repo.WORKFLOW.md",
    "generated.claude.CLAUDE.md",
    "generated.codex.AGENTS.md",
    "generated.omp.AGENTS.md",
    "generated.zcode.AGENTS.md",
    "generated.antigravity.GEMINI.md",
    "skills.inline_catalog",
    "skills.full_bodies",
    "pointer.planner",
    "pointer.research",
    "pointer.ticket_checklist",
    "pointer.warning_status",
    "pointer.ui_verification",
];

/// Surfaces whose ledger budget is the packer's hard page limit rather than the
/// ratified `ceil(actual × 1.10)` headroom rule. The emitted page is bounded by
/// this limit at runtime, so reporting a derived headroom number would
/// understate the constraint the server actually enforces.
const PACKER_BOUND_SURFACES: &[&str] = &["mcp.tools_list.handshake"];

/// The packer's hard `tools/list` page budget for the default profile, read from
/// the runtime policy owner rather than restated here.
fn packer_page_budget() -> u64 {
    let home = isolated_home("packer-budget");
    let root = repository_root();
    let output = keel_command(&home.0)
        .args([
            "stats",
            "context",
            "--json",
            "--workspace-root",
            root.to_str().expect("UTF-8 repository root"),
        ])
        .output()
        .expect("run keel stats context --json");
    assert!(
        output.status.success(),
        "keel stats context --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("parse stats context JSON");
    payload["policy"]["maxToolCatalogTokens"]
        .as_u64()
        .expect("stats context policy.maxToolCatalogTokens")
}

struct TestDirectory(PathBuf);

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("workspace repository root")
        .to_path_buf()
}

fn isolated_home(suffix: &str) -> TestDirectory {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let path = std::env::temp_dir().join(format!(
        "keel-fixed-context-{}-{nonce}-{suffix}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("create isolated keel home");
    TestDirectory(path)
}

fn keel_command(home: &Path) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("keel"));
    command
        .timeout(Duration::from_secs(30))
        .env("KEEL_HOME", home)
        .env("CLAUDE_TARGET_OVERRIDE", home);
    command
}

fn stats_json(home: &Path) -> Value {
    let root = repository_root();
    let output = keel_command(home)
        .args([
            "stats",
            "--json",
            "--claude-home",
            home.to_str().expect("UTF-8 test home"),
            "--workspace-root",
            root.to_str().expect("UTF-8 repository root"),
        ])
        .output()
        .expect("run keel stats --json");
    assert!(
        output.status.success(),
        "keel stats --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("parse keel stats JSON")
}

fn fixed_context_ledger(home: &Path) -> Value {
    stats_json(home)
        .get("fixedContextLedger")
        .cloned()
        .expect("stats JSON must include fixedContextLedger")
}

fn required_u64(document: &Value, field: &str) -> u64 {
    document
        .get(field)
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("{field} integer"))
}

fn required_str<'a>(document: &'a Value, field: &str) -> &'a str {
    document
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{field} string"))
}

fn panic_text(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    "non-string panic".to_string()
}

fn assert_within_budget(surface: &str, actual: u64, budget: u64, reproduction: &str) {
    assert!(
        actual <= budget,
        "fixed-context budget exceeded: surface={surface}, actualTokens={actual}, budgetTokens={budget}, reproductionCommand={reproduction}"
    );
}

#[derive(Clone, Copy)]
struct FixedContextRow<'a> {
    surface: &'a str,
    actual: u64,
    budget: u64,
    reproduction: &'a str,
    status: &'a str,
}

fn fixed_context_row(entry: &Value) -> FixedContextRow<'_> {
    FixedContextRow {
        surface: required_str(entry, "surface"),
        actual: required_u64(entry, "actualTokens"),
        budget: required_u64(entry, "budgetTokens"),
        reproduction: required_str(entry, "reproductionCommand"),
        status: required_str(entry, "status"),
    }
}

fn first_fixed_context_budget(home: &Path) -> (String, u64, String) {
    let ledger = fixed_context_ledger(home);
    let entry = ledger["entries"]
        .as_array()
        .expect("ledger entries")
        .first()
        .expect("at least one ledger row");
    let row = fixed_context_row(entry);
    (
        row.surface.to_owned(),
        row.budget,
        row.reproduction.to_owned(),
    )
}

fn with_ten_percent_headroom(actual: u64) -> u64 {
    actual.saturating_mul(110).saturating_add(99) / 100
}

fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut rendered = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            rendered.push(',');
        }
        rendered.push(character);
    }
    rendered
}

#[test]
fn fixed_context_ledger_recomputes_every_ratified_surface() {
    let home = isolated_home("ledger");
    let ledger = fixed_context_ledger(&home.0);
    assert_eq!(required_str(&ledger, "tokenizer"), "o200k_base");
    let entries = ledger
        .get("entries")
        .and_then(Value::as_array)
        .expect("fixedContextLedger.entries array");
    assert_eq!(
        entries.len(),
        EXPECTED_SURFACES.len(),
        "ledger row count must match the Phase 1 surface contract"
    );
    let by_surface: BTreeMap<&str, FixedContextRow<'_>> = entries
        .iter()
        .map(|entry| {
            let row = fixed_context_row(entry);
            (row.surface, row)
        })
        .collect();

    assert_eq!(
        by_surface.len(),
        EXPECTED_SURFACES.len(),
        "ledger must contain exactly the Phase 1 fixed-context surfaces"
    );
    for surface in EXPECTED_SURFACES {
        let row = by_surface
            .get(surface)
            .unwrap_or_else(|| panic!("missing fixed-context ledger surface: {surface}"));
        assert!(
            !row.reproduction.trim().is_empty(),
            "{surface} reproduction command"
        );
        assert_ne!(
            row.status,
            "source_missing",
            "fixed-context source is missing: surface={surface}, actualTokens={}, budgetTokens={}, reproductionCommand={}",
            row.actual,
            row.budget,
            row.reproduction
        );
        assert_within_budget(surface, row.actual, row.budget, row.reproduction);
        if PACKER_BOUND_SURFACES.contains(surface) {
            // §10.1: this row's budget is the hard page limit the packer enforces,
            // not a derived soft target that would understate the constraint.
            assert_eq!(
                row.budget,
                packer_page_budget(),
                "packer-bound surface must report the packer's own hard limit: surface={surface}"
            );
        } else {
            assert_eq!(
                row.budget,
                with_ten_percent_headroom(row.actual),
                "fixed-context budget is not the ratified runtime measurement plus 10% headroom: surface={surface}, actualTokens={}, budgetTokens={}, reproductionCommand={}",
                row.actual,
                row.budget,
                row.reproduction
            );
        }
        assert_eq!(row.status, "within_budget", "{surface} status");
    }
    assert_eq!(required_str(&ledger, "status"), "within_budget");

    let mcp = ledger.get("mcp").expect("MCP ledger metadata");
    let tool_count = required_u64(mcp, "toolCount");
    let eager = required_u64(mcp, "eagerToolCount");
    let deferred = required_u64(mcp, "deferredToolCount");
    assert!(tool_count > 0, "MCP catalog must expose real tools");
    assert_eq!(eager + deferred, tool_count);
    assert_eq!(eager, 17, "Tiered MCP catalog advertises 17 eager tools");
    assert_eq!(
        deferred, 20,
        "Tiered MCP catalog defers 20 specialist tools"
    );
    assert_eq!(tool_count, 37, "Total MCP catalog tools count is 37");

    let warning = by_surface["pointer.warning_status"];
    assert!(
        warning.actual < 30,
        "warning-status pointer must remain below 30 tokens"
    );
}

#[test]
fn scratch_inflation_proves_the_budget_gate_fails_actionably() {
    let home = isolated_home("scratch-inflation");
    let (surface, budget, reproduction) = first_fixed_context_budget(&home.0);

    let failure = std::panic::catch_unwind(|| {
        assert_within_budget(&surface, budget + 1, budget, &reproduction)
    })
    .expect_err("a one-token scratch inflation must fail the gate");
    let message = panic_text(failure);
    for required in [
        format!("surface={surface}"),
        format!("actualTokens={}", budget + 1),
        format!("budgetTokens={budget}"),
        format!("reproductionCommand={reproduction}"),
    ] {
        assert!(
            message.contains(&required),
            "failure must include `{required}`: {message}"
        );
    }
}

#[test]
fn stats_text_prints_the_full_fixed_context_ledger() {
    let home = isolated_home("stats-text");
    let root = repository_root();
    let output = keel_command(&home.0)
        .args([
            "stats",
            "--claude-home",
            home.0.to_str().expect("UTF-8 test home"),
            "--workspace-root",
            root.to_str().expect("UTF-8 repository root"),
        ])
        .output()
        .expect("run keel stats");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("fixed context ledger (o200k_base)"));
    for surface in EXPECTED_SURFACES {
        assert!(stdout.contains(surface), "stats text missing {surface}");
    }
}

#[test]
fn gain_separates_fixed_context_from_command_compaction_savings() {
    let home = isolated_home("gain-separation");
    let output = keel_command(&home.0)
        .args(["gain", "--json"])
        .output()
        .expect("run keel gain --json");
    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("parse gain JSON");
    let accounting = payload
        .get("fixedContextAccounting")
        .expect("gain JSON fixedContextAccounting");
    assert_eq!(
        accounting
            .get("includedInCommandCompactionSavings")
            .and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        required_str(accounting, "source"),
        "keel stats --json --workspace-root <repo>"
    );
    assert_eq!(required_str(accounting, "tokenizer"), "o200k_base");
    assert_eq!(
        required_u64(accounting, "surfaceCount"),
        EXPECTED_SURFACES.len() as u64
    );

    let text = keel_command(&home.0)
        .arg("gain")
        .output()
        .expect("run keel gain");
    assert!(text.status.success());
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(stdout.contains("Fixed context: excluded from command-compaction savings"));
    assert!(stdout.contains("keel stats --json --workspace-root <repo>"));
}

#[test]
fn fixed_context_documentation_matches_the_runtime_ledger() {
    let home = isolated_home("doc-parity");
    let documented_ledger = fixed_context_ledger(&home.0);
    let documentation_path = repository_root()
        .join("docs")
        .join("fixed-context-ledger.md");
    let documentation = std::fs::read_to_string(&documentation_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", documentation_path.display()));

    for entry in documented_ledger["entries"]
        .as_array()
        .expect("ledger entries")
    {
        let documented = fixed_context_row(entry);
        let row = format!(
            "| `{}` | {} | {} |",
            documented.surface,
            grouped(documented.actual),
            grouped(documented.budget)
        );
        assert!(
            documentation.contains(&row),
            "fixed-context documentation is stale; expected row: {row}"
        );
    }

    let mcp = &documented_ledger["mcp"];
    let mcp_summary = format!(
        "The MCP baseline is {} total tools: {} eager and {} deferred.",
        required_u64(mcp, "toolCount"),
        required_u64(mcp, "eagerToolCount"),
        required_u64(mcp, "deferredToolCount")
    );
    assert!(documentation.contains(&mcp_summary), "{mcp_summary}");
    let skill_summary = format!(
        "The skill baseline covers {} parseable first-party",
        required_u64(&documented_ledger["skills"], "skillCount")
    );
    assert!(documentation.contains(&skill_summary), "{skill_summary}");
}
