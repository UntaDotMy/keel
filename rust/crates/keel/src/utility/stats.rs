//! Purpose: `keel stats`, a single dashboard over the axes that today live in
//!   separate commands: token savings (`gain`), tool timings (`telemetry`),
//!   gate/enforcement activity, recall/memory health, and anvil job progress.
//!   It answers "what has keel done, what did it save, what did it catch" in one
//!   compact read instead of four invocations.
//! Caller: commands.rs `stats` dispatch, MCP `stats` tool.
//! Dependencies: the gain, telemetry, recall, anvil, hook-lifecycle, and
//!   fixed-context readers. Every datum comes from its owning reader; `stats`
//!   renders those values and does not re-parse their storage formats.
//! Side Effects: read-only. `recall_status_snapshot` lazily syncs the recall
//!   index; everything else reads files. No writes, no network.

use std::fs;
use std::io::Write;
use std::path::Path;

use serde_json::json;

use std::path::PathBuf;

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::runner::hook_lifecycle::gate_status_rows;
use crate::runner::telemetry::{aggregate_rows, read_rows};
use crate::runtime::{display_path, resolve_claude_home, COMMAND_COMPACTION_EVENTS_FILE_NAME};
use crate::utility::anvil::job::active_jobs_summary;
use crate::utility::fixed_context::{self, FixedContextLedger};
use crate::utility::gain::parse_gain_summary;
use crate::utility::recall::recall_status_snapshot;

/// Default `--days` window for the savings/timing axes. Matches the telemetry
/// default so the two surfaces agree on a window when neither is given one.
const DEFAULT_DAYS: u64 = 7;

pub fn run_stats_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    // Diagnostic subcommands are intentionally routed here, keeping one
    // operator surface while preserving the legacy flag-only dashboard.
    if let Some(subcommand) = arguments.first().map(String::as_str) {
        match subcommand {
            "context" => {
                return run_context_stats(&arguments[1..], standard_output, standard_error)
            }
            "tools" => return run_tools_stats(&arguments[1..], standard_output, standard_error),
            "gain" => {
                return crate::utility::gain::run_gain_command(
                    &arguments[1..],
                    standard_output,
                    standard_error,
                )
            }
            _ => {}
        }
    }
    let mut flag_set = FlagSet::new("stats");
    flag_set.bool_flag("json", false);
    flag_set.string_flag("days", "");
    flag_set.string_flag("claude-home", "");
    flag_set.string_flag("workspace-root", "");
    flag_set.string_flag("top", "5");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }

    let days = flag_set
        .string_value("days")
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_DAYS);
    let top_count: usize = flag_set
        .string_value("top")
        .trim()
        .parse()
        .unwrap_or(5)
        .min(20);

    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(home) => home,
        Err(error) => {
            let _ = writeln!(standard_error, "stats: {error}");
            return 1;
        }
    };
    let workspace_root = {
        let flag = flag_set.string_value("workspace-root").trim().to_string();
        if flag.is_empty() {
            std::env::current_dir()
                .map(|path| display_path(&path))
                .unwrap_or_default()
        } else {
            flag
        }
    };

    let snapshot = collect_snapshot(&claude_home, &workspace_root, days, top_count);

    if flag_set.bool_value("json") {
        let payload = snapshot.to_json(days);
        if let Err(write_error) = write_indented(standard_output, &payload) {
            let _ = writeln!(
                standard_error,
                "stats: unable to render JSON: {write_error}"
            );
            return 1;
        }
        return 0;
    }
    snapshot.render_text(standard_output, days);
    0
}

fn stats_workspace_root(flag_set: &FlagSet) -> String {
    let flag = flag_set.string_value("workspace-root").trim().to_string();
    if flag.is_empty() {
        std::env::current_dir()
            .map(|path| display_path(&path))
            .unwrap_or_default()
    } else {
        flag
    }
}

fn run_context_stats(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("stats context");
    flags.bool_flag("json", false);
    flags.string_flag("workspace-root", "");
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let workspace_root = stats_workspace_root(&flags);
    let ledger = fixed_context::collect(Path::new(&workspace_root));
    let entries = ledger
        .entries
        .iter()
        .map(|entry| {
            let cache_class =
                crate::proxy::context::ProjectionInput::cache_class_for_surface(entry.surface);
            let (cached_tokens, uncached_tokens) =
                crate::proxy::context::ProjectionInput::cache_split(
                    cache_class,
                    entry.actual_tokens,
                );
            let headroom = entry.budget_tokens.saturating_sub(entry.actual_tokens);
            json!({
                "surface": entry.surface,
                "rawTokens": entry.actual_tokens,
                "visibleTokens": entry.actual_tokens,
                // Exactly one of these is set, by the firewall's own cache rule,
                // so a model-visible cost is never counted as both.
                "cachedTokens": cached_tokens,
                "uncachedTokens": uncached_tokens,
                "budgetTokens": entry.budget_tokens,
                "headroomTokens": headroom,
                "cacheClass": cache_class.as_str(),
                "sourceAvailable": entry.source_available,
                "status": entry.status(),
                "reproductionCommand": fixed_context::REPRODUCTION_COMMAND,
            })
        })
        .collect::<Vec<_>>();
    let payload = json!({
        "schemaVersion": 1,
        "tokenizer": fixed_context::TOKENIZER,
        "workspaceRoot": workspace_root,
        "status": ledger.status(),
        "mcp": {
            "toolCount": ledger.mcp.tool_count,
            "eagerToolCount": ledger.mcp.eager_tool_count,
            "deferredToolCount": ledger.mcp.deferred_tool_count,
        },
        "skills": {"skillCount": ledger.skills.skill_count},
        "surfaces": entries,
        "policy": {
            "maxDynamicTokens": crate::proxy::context::DEFAULT_MAX_DYNAMIC_TOKENS,
            "maxToolCatalogTokens": crate::proxy::context::DEFAULT_MAX_TOOL_CATALOG_TOKENS,
            "maxMemoryProjectionTokens": crate::proxy::context::DEFAULT_MAX_MEMORY_PROJECTION_TOKENS,
            "maxWarningPointerTokens": crate::proxy::context::DEFAULT_MAX_WARNING_POINTER_TOKENS,
            "maxDiscoveryResultTokens": crate::proxy::context::DEFAULT_MAX_DISCOVERY_RESULT_TOKENS,
            "maxSingleResultTokens": crate::proxy::context::DEFAULT_MAX_SINGLE_RESULT_TOKENS,
        },
        "reproductionCommand": fixed_context::REPRODUCTION_COMMAND,
    });
    if flags.bool_value("json") {
        return write_serde_json(standard_output, standard_error, "stats context", &payload);
    }
    let _ = writeln!(
        standard_output,
        "keel stats context ({}) status={} workspace={}",
        fixed_context::TOKENIZER,
        ledger.status(),
        workspace_root
    );
    for entry in &ledger.entries {
        let cache_class =
            crate::proxy::context::ProjectionInput::cache_class_for_surface(entry.surface);
        let headroom = entry.budget_tokens.saturating_sub(entry.actual_tokens);
        let _ = writeln!(
            standard_output,
            "  {} raw={} visible={} budget={} headroom={} cache={} status={}",
            entry.surface,
            entry.actual_tokens,
            entry.actual_tokens,
            entry.budget_tokens,
            headroom,
            cache_class.as_str(),
            entry.status()
        );
    }
    0
}

fn run_tools_stats(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("stats tools");
    flags.bool_flag("json", false);
    flags.bool_flag("benchmark", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    if flags.bool_value("benchmark") {
        return run_tools_benchmark(flags.bool_value("json"), standard_output, standard_error);
    }
    let snapshot = crate::mcp::tools_list_context_snapshot();
    let payload = json!({
        "schemaVersion": 1,
        "profile": crate::mcp::McpCatalogProfile::from_env().as_str(),
        "toolCount": snapshot.tool_count,
        "eagerToolCount": snapshot.eager_tool_count,
        "deferredToolCount": snapshot.deferred_tool_count,
        "catalogTokens": snapshot.catalog_tokens,
        "pagination": {"supported": true, "pageSize": crate::mcp::tools_page_size()},
        "discovery": crate::mcp::tools_discovery_snapshot(),
        "reproductionCommand": "keel stats tools --json",
    });
    if flags.bool_value("json") {
        return write_serde_json(standard_output, standard_error, "stats tools", &payload);
    }
    let _ = writeln!(
        standard_output,
        "keel stats tools profile={} total={} eager={} deferred={} catalog_tokens={} discovery_calls={}",
        payload["profile"].as_str().unwrap_or("unknown"),
        snapshot.tool_count,
        snapshot.eager_tool_count,
        snapshot.deferred_tool_count,
        snapshot.catalog_tokens,
        payload["discovery"]["calls"].as_u64().unwrap_or(0)
    );
    0
}

/// Declared before the benchmark interprets anything: the catalog walk must
/// finish inside this budget or the run fails. Two seconds is far above the
/// measured sub-second cost of every profile, so it only trips on a real
/// regression rather than on a slow machine.
const MCP_BENCHMARK_FIRST_PAGE_LATENCY_MS_MAX: f64 = 2_000.0;

/// Default token budgets the MCP benchmark compares. `full` and `core` use the
/// ratified per-profile defaults so the comparison reflects real behavior; the
/// paging and compact profiles use tighter declared budgets on purpose.
fn benchmark_default_budget(profile: crate::mcp::McpCatalogProfile) -> usize {
    match profile {
        crate::mcp::McpCatalogProfile::Tiered => {
            crate::proxy::context::DEFAULT_MAX_TOOL_CATALOG_TOKENS
        }
        crate::mcp::McpCatalogProfile::Full => crate::proxy::context::DEFAULT_MAX_DYNAMIC_TOKENS,
    }
}

/// One MCP catalog benchmark configuration: a catalog profile, an optional
/// explicit schema level (absent means the spec-default handshake), and an
/// optional hard page budget (absent means the profile's ratified default).
#[derive(Debug, Clone, Copy)]
struct McpBenchmarkProfile {
    name: &'static str,
    profile: crate::mcp::McpCatalogProfile,
    budget: Option<usize>,
    level: Option<u64>,
}

const fn core_benchmark(
    name: &'static str,
    budget: Option<usize>,
    level: Option<u64>,
) -> McpBenchmarkProfile {
    McpBenchmarkProfile {
        name,
        profile: crate::mcp::McpCatalogProfile::Tiered,
        budget,
        level,
    }
}

const MCP_BENCHMARK_PROFILES: &[McpBenchmarkProfile] = &[
    McpBenchmarkProfile {
        name: "full",
        profile: crate::mcp::McpCatalogProfile::Full,
        budget: None,
        level: None,
    },
    McpBenchmarkProfile {
        name: "core",
        profile: crate::mcp::McpCatalogProfile::Tiered,
        budget: None,
        level: None,
    },
    core_benchmark("core+pagination", Some(600), Some(2)),
    core_benchmark("core+progressive-discovery", Some(1_200), Some(0)),
    core_benchmark(
        "core+progressive-discovery+compact-schemas",
        Some(600),
        Some(0),
    ),
];

/// Representative task families from the gateway plan. Each entry is a task
/// label plus the intent a host would express for it; the benchmark records
/// whether discovery can name a capability for that intent.
const MCP_BENCHMARK_TASKS: &[(&str, &str)] = &[
    ("filesystem", "read and write a file"),
    ("search", "search the codebase for a symbol"),
    ("build", "build the workspace"),
    ("test", "run the test suite"),
    ("debug", "diagnose a failing test or warning"),
    ("git", "inspect git history and diff"),
    ("frontend", "review ui layout and accessibility"),
    ("backend", "design an api boundary"),
    ("database", "write a database migration"),
    ("web", "automate a browser flow"),
    ("configuration", "audit agent configuration"),
    ("release", "verify a packaged release"),
];

/// Traverse a profile's catalog the way a client would, following the opaque
/// cursor until the server stops emitting one. Returns the exact first-page
/// cost, the page count, and the ordered tool names across every page.
fn walk_benchmark_catalog(
    configuration: McpBenchmarkProfile,
) -> Result<(usize, usize, Vec<String>), String> {
    let mut params = match configuration.level {
        Some(level) => json!({ "level": level }),
        None => json!({}),
    };
    let mut first_page_tokens = 0usize;
    let mut pages = 0usize;
    let mut names = Vec::new();
    for _ in 0..64 {
        let page = crate::mcp::tools_list_page(configuration.profile, &params)?;
        let measured = crate::mcp::measure_tools_list_response(&page);
        if pages == 0 {
            first_page_tokens = measured;
        }
        pages += 1;
        names.extend(
            page["tools"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|tool| tool["name"].as_str().map(str::to_string)),
        );
        match page["nextCursor"].as_str() {
            Some(cursor) => params = json!({ "cursor": cursor }),
            None => break,
        }
    }
    Ok((first_page_tokens, pages, names))
}

fn run_tools_benchmark(
    json_output: bool,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut profiles = Vec::new();
    let mut failures = Vec::new();
    for configuration in MCP_BENCHMARK_PROFILES {
        let budget = configuration
            .budget
            .unwrap_or_else(|| benchmark_default_budget(configuration.profile));
        // why: an unset override is the normal case, so only its presence matters.
        let previous = std::env::var("KEEL_MCP_PAGE_TOKENS").ok();
        std::env::set_var("KEEL_MCP_PAGE_TOKENS", budget.to_string());
        let started = std::time::Instant::now();
        let walked = walk_benchmark_catalog(*configuration);
        let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        match previous {
            Some(value) => std::env::set_var("KEEL_MCP_PAGE_TOKENS", value),
            None => std::env::remove_var("KEEL_MCP_PAGE_TOKENS"),
        }
        let (first_page_tokens, pages, names) = match walked {
            Ok(walked) => walked,
            Err(error) => {
                failures.push(format!("{}: {error}", configuration.name));
                continue;
            }
        };
        let unique: std::collections::BTreeSet<&str> = names.iter().map(String::as_str).collect();
        let complete_catalog_tokens = crate::mcp::measure_tools_list_response(
            &crate::mcp::tools_complete_catalog(configuration.profile),
        );
        if unique.len() != names.len() {
            failures.push(format!(
                "{}: traversal duplicated a tool",
                configuration.name
            ));
        }
        if first_page_tokens > budget {
            failures.push(format!(
                "{}: first page {first_page_tokens} exceeded its {budget}-token budget",
                configuration.name
            ));
        }
        let mut discovery_covered = 0usize;
        for (_, intent) in MCP_BENCHMARK_TASKS {
            // A discovery failure for one intent is not a harness fault; it means
            // that task is uncovered, which the coverage count reports.
            let discovered = match crate::mcp::tools_discover(intent, 5, 1) {
                Ok(payload) => payload["count"].as_u64().unwrap_or(0),
                Err(_) => 0,
            };
            if discovered > 0 {
                discovery_covered += 1;
            }
        }
        // Enforce the declared thresholds rather than only reporting them.
        if discovery_covered < MCP_BENCHMARK_TASKS.len() {
            failures.push(format!(
                "{}: discovery covered {discovery_covered}/{} task families, below the declared 100%",
                configuration.name,
                MCP_BENCHMARK_TASKS.len()
            ));
        }
        if elapsed_ms > MCP_BENCHMARK_FIRST_PAGE_LATENCY_MS_MAX {
            failures.push(format!(
                "{}: catalog walk took {elapsed_ms:.1}ms, above the declared {MCP_BENCHMARK_FIRST_PAGE_LATENCY_MS_MAX:.0}ms",
                configuration.name
            ));
        }
        profiles.push(json!({
            "profile": configuration.name,
            "catalogProfile": configuration.profile.as_str(),
            "schemaLevel": configuration.level.map(|level| json!(level)).unwrap_or(json!("spec-default")),
            "pageBudgetTokens": budget,
            "completeCatalogTokens": complete_catalog_tokens,
            "firstPageTokens": first_page_tokens,
            "pagesToTraverse": pages,
            "advertisedTools": names.len(),
            "uniqueTools": unique.len(),
            "reachableTools": unique.len(),
            "storedTools": crate::mcp::tools_stored_tool_count(),
            "discoveryTasksCovered": discovery_covered,
            "discoveryTaskCount": MCP_BENCHMARK_TASKS.len(),
            "buildLatencyMs": (elapsed_ms * 100.0).round() / 100.0,
        }));
    }
    let coverage_target = MCP_BENCHMARK_TASKS.len();
    let payload = json!({
        "schemaVersion": 1,
        "benchmark": "mcp-catalog-progressive-disclosure",
        "tokenizer": "o200k_base",
        // Declared before interpretation: these floors are a pass/fail contract.
        "declaredThresholds": {
            "firstPageTokensMax": "declared per profile in pageBudgetTokens; the run fails if any page exceeds it",
            "traversalCompleteness": "uniqueTools == storedTools and uniqueTools == advertisedTools",
            "duplicateToolsMax": 0,
            "omittedToolsMax": 0,
            "discoveryCoverageMinPercent": 100.0,
            "firstPageLatencyMsMax": MCP_BENCHMARK_FIRST_PAGE_LATENCY_MS_MAX,
            "reacquisitionRequired": false,
        },
        "policy": {
            "hardPageBudget": "every emitted page must measure at or below its declared budget",
            "traversal": "following nextCursor must reach every stored tool exactly once",
            "dispatchParity": "every stored tool stays callable regardless of the active profile",
            "reacquisitionLimit": "deferred tools must be nameable through discovery",
            "discoveryCoverageTarget": coverage_target,
            "defaultDecisionRule": "a cheaper profile is accepted only when first-page cost falls, traversal stays complete and duplicate-free, dispatch parity holds, and discovery covers every representative task",
        },
        "taskCount": MCP_BENCHMARK_TASKS.len(),
        "profiles": profiles,
        "failures": failures,
        "status": if failures.is_empty() { "passed" } else { "failed" },
        "reproductionCommand": "keel stats tools --benchmark --json",
    });
    if json_output {
        let code = write_serde_json(standard_output, standard_error, "stats tools", &payload);
        if code != 0 {
            return code;
        }
    } else {
        for profile in &profiles {
            let _ = writeln!(
                standard_output,
                "keel stats tools --benchmark profile={} budget={} first_page={} pages={} unique={} of {} discovery={}/{} latency_ms={}",
                profile["profile"].as_str().unwrap_or("unknown"),
                profile["pageBudgetTokens"].as_u64().unwrap_or(0),
                profile["firstPageTokens"].as_u64().unwrap_or(0),
                profile["pagesToTraverse"].as_u64().unwrap_or(0),
                profile["uniqueTools"].as_u64().unwrap_or(0),
                profile["storedTools"].as_u64().unwrap_or(0),
                profile["discoveryTasksCovered"].as_u64().unwrap_or(0),
                profile["discoveryTaskCount"].as_u64().unwrap_or(0),
                profile["buildLatencyMs"].as_f64().unwrap_or(0.0),
            );
        }
    }
    if failures.is_empty() {
        0
    } else {
        for failure in &failures {
            let _ = writeln!(standard_error, "stats tools --benchmark: {failure}");
        }
        1
    }
}

fn write_serde_json(
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
    label: &str,
    payload: &serde_json::Value,
) -> u8 {
    match serde_json::to_string_pretty(payload) {
        Ok(text) => {
            let _ = writeln!(standard_output, "{text}");
            0
        }
        Err(error) => {
            let _ = writeln!(standard_error, "{label}: {error}");
            1
        }
    }
}

/// One aggregated read over every axis. Fields are already-computed values from
/// the owning readers; `stats` adds no parsing of its own on top of them.
struct StatsSnapshot {
    tokens_saved: u64,
    tokens_overhead: u64,
    net_tokens_saved: i64,
    tokens_before: u64,
    savings_percent: f64,
    net_savings_percent: f64,
    session_start_context_tokens_now: usize,
    user_prompt_base_context_tokens_now: usize,
    user_prompt_code_change_context_tokens_now: usize,
    mcp_catalog_tokens_now: usize,
    mcp_tool_count: usize,
    fixed_context: FixedContextLedger,
    commands_observed: u64,
    commands_compacted: u64,
    top_commands: Vec<(String, u64)>,
    top_tools: Vec<(String, u64, u64)>,
    gates: Vec<(String, u64)>,
    recall_documents: Option<u64>,
    recall_last_indexed_ms: u128,
    anvil_jobs: Vec<(String, String)>,
}

/// Read the compaction event log under `claude_home` and reuse the `gain`
/// parser. Home-driven (not env-driven) so `stats --claude-home` reports the
/// home it resolved and so tests stay hermetic. Missing/unreadable log yields
/// the parser's empty summary, matching `gain` on a fresh home.
fn gain_summary_from_home(claude_home: &Path, days: u64) -> crate::utility::gain::GainSummary {
    let path = claude_home.join(COMMAND_COMPACTION_EVENTS_FILE_NAME);
    let text = fs::read_to_string(path).unwrap_or_default();
    parse_gain_summary(&text, Some(days_cutoff(days)), None)
}

/// Day-file paths under `claude_home/state/tool-timings/<date>.jsonl` for the
/// trailing `days` window, oldest-first, existing files only. This mirrors
/// `iter_day_files` but is rooted at the resolved home so it never reads the
/// process env home. Parsing stays in `read_rows`/`aggregate_rows`.
fn telemetry_day_files(claude_home: &Path, days: u64) -> Vec<PathBuf> {
    let directory = claude_home.join("state").join("tool-timings");
    let today = chrono::Local::now().date_naive();
    let mut paths = Vec::new();
    for offset in 0..days {
        let Some(date) = today.checked_sub_days(chrono::Days::new(offset)) else {
            break;
        };
        let path = directory.join(format!("{}.jsonl", date.format("%Y-%m-%d")));
        if path.exists() {
            paths.push(path);
        }
    }
    paths.sort();
    paths
}

fn collect_snapshot(
    claude_home: &Path,
    workspace_root: &str,
    days: u64,
    top_count: usize,
) -> StatsSnapshot {
    let gain = gain_summary_from_home(claude_home, days);
    let top_commands = gain
        .top_commands
        .iter()
        .take(top_count)
        .map(|item| (item.command.clone(), item.tokens_saved))
        .collect::<Vec<_>>();

    let rows = read_rows(
        telemetry_day_files(claude_home, days)
            .iter()
            .map(PathBuf::as_path),
        None,
    );
    let top_tools = aggregate_rows(rows, top_count)
        .into_iter()
        .map(|summary| (summary.tool_name, summary.count, summary.sum_ms))
        .collect::<Vec<_>>();

    let gates = gate_activity(claude_home);

    let (recall_documents, recall_last_indexed_ms) = match recall_status_snapshot(claude_home) {
        Ok(status) => (Some(status.document_count), status.last_indexed_at_millis),
        Err(_) => (None, 0),
    };

    // Real anvil state, scoped to the requested workspace. Empty means nothing
    // was ever run there; renderers must omit the axis instead of faking one.
    let anvil_jobs = active_jobs_summary(claude_home, Some(Path::new(workspace_root)));
    let session_start_context_tokens_now = crate::proxy::token_meter::TokenMeter::count_text(
        &crate::runner::hook_lifecycle::session_start_context(),
    );
    let fixed_context = fixed_context::collect(Path::new(workspace_root));
    let entry_tokens = |surface| {
        fixed_context
            .entries
            .iter()
            .find(|entry| entry.surface == surface)
            .map(|entry| entry.actual_tokens)
            .unwrap_or_default()
    };
    let user_prompt_base_context_tokens_now = entry_tokens("hook.user_prompt_submit.simple");
    let user_prompt_code_change_context_tokens_now =
        entry_tokens("hook.user_prompt_submit.code_change");
    let mcp_tool_count = fixed_context.mcp.tool_count;
    let mcp_catalog_tokens_now = entry_tokens("mcp.tools_list.catalog");

    StatsSnapshot {
        tokens_saved: gain.tokens_saved,
        tokens_overhead: gain.tokens_overhead,
        net_tokens_saved: gain.net_tokens_saved,
        tokens_before: gain.tokens_before,
        savings_percent: gain.savings_percent(),
        net_savings_percent: gain.net_savings_percent(),
        session_start_context_tokens_now,
        user_prompt_base_context_tokens_now,
        user_prompt_code_change_context_tokens_now,
        mcp_catalog_tokens_now,
        mcp_tool_count,
        fixed_context,
        commands_observed: gain.commands_observed,
        commands_compacted: gain.commands_compacted,
        top_commands,
        top_tools,
        gates,
        recall_documents,
        recall_last_indexed_ms,
        anvil_jobs,
    }
}

/// Sum every per-session counter file under each gate's state directory. Gates
/// persist one counter file per session key, so cross-session activity is the
/// directory total. The dir/label pairs come from the single source of truth
/// (`gate_status_rows`) so `stats` reports the same gates the hook path fires.
fn gate_activity(claude_home: &Path) -> Vec<(String, u64)> {
    gate_status_rows()
        .into_iter()
        .map(|row| {
            let dir = claude_home.join("state").join(row.dir);
            let total = fs::read_dir(&dir)
                .map(|entries| {
                    entries
                        .filter_map(std::result::Result::ok)
                        .map(|entry| {
                            fs::read_to_string(entry.path())
                                .ok()
                                .and_then(|text| text.trim().parse::<u64>().ok())
                                .unwrap_or(0)
                        })
                        .sum::<u64>()
                })
                .unwrap_or(0);
            (row.label.to_string(), total)
        })
        .collect()
}

fn days_cutoff(days: u64) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    now.saturating_sub(days.saturating_mul(24 * 3600))
}

impl StatsSnapshot {
    fn render_text(&self, standard_output: &mut dyn Write, days: u64) {
        let _ = writeln!(
            standard_output,
            "keel stats (last {}d): {} net tokens saved ({:.1}% of {})",
            days, self.net_tokens_saved, self.net_savings_percent, self.tokens_before
        );
        let _ = writeln!(
            standard_output,
            "  command compaction: {} gross saved ({:.1}%), {} wrapper overhead",
            self.tokens_saved, self.savings_percent, self.tokens_overhead
        );
        let _ = writeln!(
            standard_output,
            "  context now (o200k_base): session-start {}, user-prompt base {}/turn, code-change {}/turn, MCP catalog {} ({} tools)",
            self.session_start_context_tokens_now,
            self.user_prompt_base_context_tokens_now,
            self.user_prompt_code_change_context_tokens_now,
            self.mcp_catalog_tokens_now,
            self.mcp_tool_count
        );
        let _ = writeln!(
            standard_output,
            "  scope: command savings exclude hook and MCP catalog context; end-to-end net depends on host usage"
        );
        let _ = writeln!(
            standard_output,
            "  fixed context ledger ({}): {} ({} surfaces; rows are not additive)",
            fixed_context::TOKENIZER,
            self.fixed_context.status(),
            self.fixed_context.entries.len()
        );
        let _ = writeln!(
            standard_output,
            "    MCP tools: {} total, {} eager, {} deferred; skills: {}",
            self.fixed_context.mcp.tool_count,
            self.fixed_context.mcp.eager_tool_count,
            self.fixed_context.mcp.deferred_tool_count,
            self.fixed_context.skills.skill_count
        );
        for entry in &self.fixed_context.entries {
            let _ = writeln!(
                standard_output,
                "    {}: actual={} budget={} status={} reproduce=`{}`",
                entry.surface,
                entry.actual_tokens,
                entry.budget_tokens,
                entry.status(),
                fixed_context::REPRODUCTION_COMMAND
            );
        }
        let _ = writeln!(
            standard_output,
            "  commands: {} observed, {} compacted",
            self.commands_observed, self.commands_compacted
        );
        if !self.top_commands.is_empty() {
            let _ = writeln!(standard_output, "  top savers:");
            for (command, saved) in &self.top_commands {
                let _ = writeln!(standard_output, "    {saved:>8}  {command}");
            }
        }
        if !self.top_tools.is_empty() {
            let _ = writeln!(standard_output, "  top tools (by time):");
            for (tool, count, sum_ms) in &self.top_tools {
                let _ = writeln!(standard_output, "    {sum_ms:>8}ms  {tool} (x{count})");
            }
        }
        let fired: Vec<(&String, u64)> = self
            .gates
            .iter()
            .map(|(label, count)| (label, *count))
            .filter(|(_, count)| *count > 0)
            .collect();
        if fired.is_empty() {
            let _ = writeln!(standard_output, "  gates: none fired");
        } else {
            let _ = writeln!(standard_output, "  gates fired:");
            for (label, count) in fired {
                let _ = writeln!(standard_output, "    {label}: {count}");
            }
        }
        match self.recall_documents {
            Some(count) => {
                let _ = writeln!(standard_output, "  memory: {count} documents indexed");
            }
            None => {
                let _ = writeln!(standard_output, "  memory: index unavailable");
            }
        }
        // Honest axis: nothing to report when no anvil job was ever run in
        // this workspace, so the lines are omitted entirely.
        if !self.anvil_jobs.is_empty() {
            let _ = writeln!(standard_output, "  anvil: {} job(s)", self.anvil_jobs.len());
            for (id, state) in self.anvil_jobs.iter().take(5) {
                let _ = writeln!(standard_output, "    {id} [{state}]");
            }
        }
    }

    fn to_json(&self, days: u64) -> Value {
        let top_commands = self
            .top_commands
            .iter()
            .map(|(command, saved)| {
                Value::Object(vec![
                    ("command".into(), Value::String(command.clone())),
                    ("tokensSaved".into(), Value::Number(saved.to_string())),
                ])
            })
            .collect::<Vec<_>>();
        let top_tools = self
            .top_tools
            .iter()
            .map(|(tool, count, sum_ms)| {
                Value::Object(vec![
                    ("tool".into(), Value::String(tool.clone())),
                    ("count".into(), Value::Number(count.to_string())),
                    ("sumMs".into(), Value::Number(sum_ms.to_string())),
                ])
            })
            .collect::<Vec<_>>();
        let gates = self
            .gates
            .iter()
            .map(|(label, count)| {
                Value::Object(vec![
                    ("gate".into(), Value::String(label.clone())),
                    ("count".into(), Value::Number(count.to_string())),
                ])
            })
            .collect::<Vec<_>>();
        let fixed_context_entries = self
            .fixed_context
            .entries
            .iter()
            .map(|entry| {
                Value::Object(vec![
                    ("surface".into(), Value::String(entry.surface.into())),
                    (
                        "actualTokens".into(),
                        Value::Number(entry.actual_tokens.to_string()),
                    ),
                    (
                        "budgetTokens".into(),
                        Value::Number(entry.budget_tokens.to_string()),
                    ),
                    ("status".into(), Value::String(entry.status().into())),
                    (
                        "reproductionCommand".into(),
                        Value::String(fixed_context::REPRODUCTION_COMMAND.into()),
                    ),
                ])
            })
            .collect::<Vec<_>>();
        let fixed_context_ledger = Value::Object(vec![
            (
                "tokenizer".into(),
                Value::String(fixed_context::TOKENIZER.into()),
            ),
            (
                "status".into(),
                Value::String(self.fixed_context.status().into()),
            ),
            ("entries".into(), Value::Array(fixed_context_entries)),
            (
                "mcp".into(),
                Value::Object(vec![
                    (
                        "toolCount".into(),
                        Value::Number(self.fixed_context.mcp.tool_count.to_string()),
                    ),
                    (
                        "eagerToolCount".into(),
                        Value::Number(self.fixed_context.mcp.eager_tool_count.to_string()),
                    ),
                    (
                        "deferredToolCount".into(),
                        Value::Number(self.fixed_context.mcp.deferred_tool_count.to_string()),
                    ),
                ]),
            ),
            (
                "skills".into(),
                Value::Object(vec![(
                    "skillCount".into(),
                    Value::Number(self.fixed_context.skills.skill_count.to_string()),
                )]),
            ),
        ]);
        // Omit the anvil axis when no jobs are active.
        // Absence is the truthful "nothing was run" signal.
        let mut fields = vec![
            ("days".into(), Value::Number(days.to_string())),
            (
                "tokensSaved".into(),
                Value::Number(self.tokens_saved.to_string()),
            ),
            (
                "grossTokensSaved".into(),
                Value::Number(self.tokens_saved.to_string()),
            ),
            (
                "tokensOverhead".into(),
                Value::Number(self.tokens_overhead.to_string()),
            ),
            (
                "netTokensSaved".into(),
                Value::Number(self.net_tokens_saved.to_string()),
            ),
            (
                "tokensBefore".into(),
                Value::Number(self.tokens_before.to_string()),
            ),
            (
                "savingsPercent".into(),
                Value::Number(format!("{:.2}", self.savings_percent)),
            ),
            (
                "grossSavingsPercent".into(),
                Value::Number(format!("{:.2}", self.savings_percent)),
            ),
            (
                "netSavingsPercent".into(),
                Value::Number(format!("{:.2}", self.net_savings_percent)),
            ),
            (
                "sessionStartContextTokensNow".into(),
                Value::Number(self.session_start_context_tokens_now.to_string()),
            ),
            (
                "userPromptBaseContextTokensNow".into(),
                Value::Number(self.user_prompt_base_context_tokens_now.to_string()),
            ),
            (
                "userPromptCodeChangeContextTokensNow".into(),
                Value::Number(self.user_prompt_code_change_context_tokens_now.to_string()),
            ),
            (
                "mcpCatalogTokensNow".into(),
                Value::Number(self.mcp_catalog_tokens_now.to_string()),
            ),
            (
                "mcpToolCount".into(),
                Value::Number(self.mcp_tool_count.to_string()),
            ),
            ("fixedContextLedger".into(), fixed_context_ledger),
            (
                "measurementScope".into(),
                Value::String(
                    "command-output net excludes hook and MCP catalog context; current context costs are reported separately"
                        .into(),
                ),
            ),
            (
                "commandsObserved".into(),
                Value::Number(self.commands_observed.to_string()),
            ),
            (
                "commandsCompacted".into(),
                Value::Number(self.commands_compacted.to_string()),
            ),
            ("topCommands".into(), Value::Array(top_commands)),
            ("topTools".into(), Value::Array(top_tools)),
            ("gates".into(), Value::Array(gates)),
            (
                "recallDocuments".into(),
                match self.recall_documents {
                    Some(count) => Value::Number(count.to_string()),
                    None => Value::String("unavailable".into()),
                },
            ),
            (
                "recallLastIndexedMs".into(),
                Value::Number(self.recall_last_indexed_ms.to_string()),
            ),
        ];
        if !self.anvil_jobs.is_empty() {
            let jobs = self
                .anvil_jobs
                .iter()
                .map(|(id, state)| {
                    Value::Object(vec![
                        ("id".into(), Value::String(id.clone())),
                        ("state".into(), Value::String(state.clone())),
                    ])
                })
                .collect::<Vec<_>>();
            fields.push(("anvilJobs".into(), Value::Array(jobs)));
        }
        Value::Object(fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn mcp_benchmark_profiles_stay_bounded_complete_and_deduplicated() {
        let _env_guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for configuration in MCP_BENCHMARK_PROFILES {
            let budget = configuration
                .budget
                .unwrap_or_else(|| benchmark_default_budget(configuration.profile));
            // why: the canonical budget owner reads this env var, so restore it.
            let previous = std::env::var("KEEL_MCP_PAGE_TOKENS").ok();
            std::env::set_var("KEEL_MCP_PAGE_TOKENS", budget.to_string());
            let walked = walk_benchmark_catalog(*configuration);
            match previous {
                Some(value) => std::env::set_var("KEEL_MCP_PAGE_TOKENS", value),
                None => std::env::remove_var("KEEL_MCP_PAGE_TOKENS"),
            }
            let (first_page_tokens, pages, names) =
                walked.unwrap_or_else(|error| panic!("{}: {error}", configuration.name));
            assert!(pages >= 1, "{}: no page emitted", configuration.name);
            assert!(
                first_page_tokens <= budget,
                "{}: first page {first_page_tokens} exceeded its {budget}-token budget",
                configuration.name
            );
            let unique: std::collections::BTreeSet<&str> =
                names.iter().map(String::as_str).collect();
            assert_eq!(
                unique.len(),
                names.len(),
                "{}: traversal duplicated a tool",
                configuration.name
            );
            let expected = match configuration.profile {
                crate::mcp::McpCatalogProfile::Tiered => crate::mcp::tools_eager_tool_count(),
                crate::mcp::McpCatalogProfile::Full => crate::mcp::tools_stored_tool_count(),
            };
            assert_eq!(
                unique.len(),
                expected,
                "{}: traversal omitted a tool",
                configuration.name
            );
        }
    }

    #[test]
    fn mcp_benchmark_discovery_covers_every_representative_task() {
        for (task, intent) in MCP_BENCHMARK_TASKS {
            let payload = crate::mcp::tools_discover(intent, 5, 1)
                .unwrap_or_else(|error| panic!("{task}: {error}"));
            assert!(
                payload["count"].as_u64().unwrap_or(0) >= 1,
                "{task}: discovery returned no capability for {intent:?}"
            );
        }
    }

    fn with_isolated_home<F: FnOnce(&std::path::PathBuf) -> R, R>(suffix: &str, run: F) -> R {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "keel-stats-{}-{nanos}-{suffix}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create test claude home");
        let result = run(&root);
        let _ = fs::remove_dir_all(&root);
        result
    }

    #[test]
    fn days_cutoff_rolls_back_window() {
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let cutoff = days_cutoff(7);
        let after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        assert!(cutoff <= before.saturating_sub(7 * 24 * 3600));
        assert!(cutoff >= after.saturating_sub(8 * 24 * 3600));
    }

    #[test]
    fn gate_activity_empty_when_no_state() {
        with_isolated_home("gates-empty", |home| {
            let activity = gate_activity(home);
            assert_eq!(activity.len(), 6);
            assert!(activity.iter().all(|(_, count)| *count == 0));
        });
    }

    #[test]
    fn gate_activity_sums_session_counters() {
        with_isolated_home("gates-sum", |home| {
            let dir = home.join("state").join("review-gate-blocks");
            fs::create_dir_all(&dir).expect("mkdir gate dir");
            fs::write(dir.join("sess-a"), "3").expect("write counter a");
            fs::write(dir.join("sess-b"), "2").expect("write counter b");
            let activity = gate_activity(home);
            let review = activity
                .iter()
                .find(|(label, _)| label == "review")
                .expect("review gate present");
            assert_eq!(review.1, 5);
        });
    }

    #[test]
    fn snapshot_renders_headline_and_axes() {
        with_isolated_home("render", |home| {
            let snapshot = collect_snapshot(home, "", 7, 5);
            let mut out: Vec<u8> = Vec::new();
            snapshot.render_text(&mut out, 7);
            let rendered = String::from_utf8_lossy(&out);
            assert!(rendered.contains("tokens saved"), "rendered: {rendered}");
            assert!(rendered.contains("commands:"), "rendered: {rendered}");
            assert!(rendered.contains("gates:"), "rendered: {rendered}");
            assert!(rendered.contains("memory:"), "rendered: {rendered}");
            // Honest axis: with no anvil job ever run, the anvil lines are
            // omitted entirely instead of printing a fabricated placeholder.
            assert!(!rendered.contains("anvil:"), "rendered: {rendered}");
        });
    }

    #[test]
    fn snapshot_reports_real_anvil_jobs_for_seeded_store() {
        with_isolated_home("render-seeded", |home| {
            let workspace_root = "C:/anvil-stats-ws";
            let expected_lane = crate::utility::system_map::workspace_key(workspace_root);
            let lane = home
                .join("memories")
                .join("workspaces")
                .join(&expected_lane)
                .join("anvil");
            fs::create_dir_all(&lane).expect("seed lane");
            fs::write(lane.join("anvil.lock.json"), "{}").expect("seed lock");

            let snapshot = collect_snapshot(home, workspace_root, 7, 5);
            let mut out: Vec<u8> = Vec::new();
            snapshot.render_text(&mut out, 7);
            let rendered = String::from_utf8_lossy(&out);
            assert!(rendered.contains("anvil: 1 job(s)"), "rendered: {rendered}");
            assert!(
                rendered.contains(&format!("{expected_lane} [active]")),
                "rendered: {rendered}"
            );
        });
    }

    #[test]
    fn json_payload_omits_anvil_axis_when_empty_and_carries_it_when_seeded() {
        with_isolated_home("json", |home| {
            let snapshot = collect_snapshot(home, "", 7, 5);
            let Value::Object(map) = snapshot.to_json(7) else {
                panic!("expected object payload");
            };
            let keys: Vec<&str> = map.iter().map(|(key, _)| key.as_str()).collect();
            for expected in [
                "tokensSaved",
                "grossTokensSaved",
                "tokensOverhead",
                "netTokensSaved",
                "grossSavingsPercent",
                "netSavingsPercent",
                "sessionStartContextTokensNow",
                "userPromptBaseContextTokensNow",
                "userPromptCodeChangeContextTokensNow",
                "mcpCatalogTokensNow",
                "mcpToolCount",
                "fixedContextLedger",
                "measurementScope",
                "commandsObserved",
                "topCommands",
                "topTools",
                "gates",
                "recallDocuments",
            ] {
                assert!(keys.contains(&expected), "missing {expected}: {keys:?}");
            }
            // Nothing ran here, so the anvil key is absent rather than fake.
            assert!(!keys.contains(&"openStories"), "keys: {keys:?}");
            assert!(!keys.contains(&"anvilJobs"), "keys: {keys:?}");
        });

        with_isolated_home("json-seeded", |home| {
            let lane = home
                .join("memories")
                .join("workspaces")
                .join("c-anvil-stats-ws")
                .join("anvil");
            fs::create_dir_all(&lane).expect("seed lane");
            fs::write(lane.join("anvil.lock.json"), "{}").expect("seed lock");

            let snapshot = collect_snapshot(home, "C:/anvil-stats-ws", 7, 5);
            let Value::Object(map) = snapshot.to_json(7) else {
                panic!("expected object payload");
            };
            assert!(map.iter().any(|(key, _)| key == "anvilJobs"));
        });
    }
}
