//! Purpose: Compiled specification, research, task, and traceability workflow.
//! Caller: commands.rs `plan` dispatch.
//! Dependencies: FlagSet, canonical workspace keys, atomic runtime writes, serde_json.
//! Main Functions: run_plan_command.
//! Side Effects: Writes versioned planner artifacts under the workspace memory lane.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};

use crate::args::FlagSet;
use crate::runtime::{
    display_path, resolve_claude_home, resolve_repository_root, safe_path_segment, write_text,
};

const SCHEMA_VERSION: u64 = 1;
const SPEC_FILE: &str = "spec.md";
const RESEARCH_FILE: &str = "research.json";
const ARCHITECTURE_FILE: &str = "architecture.md";
const TASKS_FILE: &str = "tasks.json";
const RTM_FILE: &str = "rtm.json";
const STATUS_FILE: &str = "status.json";

type Output<'a> = &'a mut dyn Write;
type Issues = Vec<String>;

const SPEC_SECTIONS: [&str; 22] = [
    "User request verbatim",
    "Restated user outcome",
    "Problem statement",
    "Scope",
    "Non-goals",
    "Constraints",
    "Stakeholders/users affected",
    "Current behavior",
    "Desired behavior",
    "Functional requirements",
    "Non-functional requirements",
    "Acceptance criteria",
    "Edge cases",
    "Failure behavior",
    "Security/privacy concerns",
    "Performance and token constraints",
    "Compatibility/host constraints",
    "Rollout and rollback requirements",
    "Assumptions",
    "Ambiguities and clarification decisions",
    "Research references",
    "Verification strategy",
];

const VAGUE_PREDICATES: [&str; 7] = [
    "works", "correct", "nice", "good", "proper", "fast", "secure",
];

macro_rules! command_or_return {
    ($expression:expr, $standard_error:expr) => {
        match $expression {
            Ok(value) => value,
            Err(error) => return command_error($standard_error, &error),
        }
    };
}

macro_rules! parsed_or_return {
    ($expression:expr) => {
        match $expression {
            Ok(value) => value,
            Err(code) => return code,
        }
    };
}

#[derive(Clone, Copy)]
enum PlanAction {
    Specify,
    Research,
    Tasks,
    Check,
}

impl PlanAction {
    fn name(self) -> &'static str {
        match self {
            Self::Specify => "plan specify",
            Self::Research => "plan research",
            Self::Tasks => "plan tasks",
            Self::Check => "plan check",
        }
    }
}

#[derive(Debug)]
struct PlannerContext {
    home: PathBuf,
    workspace_root: PathBuf,
    plans_root: PathBuf,
}

impl PlannerContext {
    fn workspace(&self) -> &Path {
        &self.workspace_root
    }
}

#[derive(Debug, Clone)]
struct PlanPaths {
    directory: PathBuf,
    spec: PathBuf,
    research: PathBuf,
    architecture: PathBuf,
    tasks: PathBuf,
    rtm: PathBuf,
    status: PathBuf,
}

impl PlanPaths {
    fn new(directory: PathBuf) -> Self {
        Self {
            spec: directory.join(SPEC_FILE),
            research: directory.join(RESEARCH_FILE),
            architecture: directory.join(ARCHITECTURE_FILE),
            tasks: directory.join(TASKS_FILE),
            rtm: directory.join(RTM_FILE),
            status: directory.join(STATUS_FILE),
            directory,
        }
    }
}

#[derive(Debug, Clone)]
struct Requirement {
    id: String,
    classification: String,
}

#[derive(Debug, Clone)]
struct AcceptanceCriterion {
    id: String,
    requirement_ids: Vec<String>,
    precondition: String,
    action: String,
    expected_outcome: String,
    negative_outcome: String,
    verification_method: String,
    evidence_type: String,
    owner_role: String,
}

#[derive(Debug, Default)]
struct ParsedSpecification {
    requirements: Vec<Requirement>,
    acceptance_criteria: Vec<AcceptanceCriterion>,
}

struct CommandStreams<'a> {
    output: Output<'a>,
    error: Output<'a>,
}

#[derive(Debug)]
struct SubmittedResearch {
    claim: String,
    source_url: String,
    source_type: String,
    publication_date: Option<String>,
    retrieved_at: String,
    support: String,
    freshness: String,
    used_by: Vec<String>,
}

#[derive(Debug)]
struct ResearchBundle {
    sources: Vec<Value>,
    claims: Vec<Value>,
    grounding: String,
    research_source: String,
    primary_source: Option<String>,
    truncated: bool,
}

pub fn run_plan_command(
    arguments: &[String],
    standard_output: Output<'_>,
    standard_error: Output<'_>,
) -> u8 {
    let Some(subcommand) = arguments.first().map(|value| value.trim()) else {
        return usage(standard_error);
    };
    let action = match subcommand {
        "specify" => PlanAction::Specify,
        "research" => PlanAction::Research,
        "tasks" => PlanAction::Tasks,
        "check" => PlanAction::Check,
        other => {
            let _ = writeln!(standard_error, "Unknown plan command: {other}");
            return usage(standard_error);
        }
    };
    let flags = parsed_or_return!(parse_action_flags(action, &arguments[1..], standard_error));
    let mut streams = CommandStreams {
        output: standard_output,
        error: standard_error,
    };
    match action {
        PlanAction::Specify => run_specify(flags, &mut streams),
        PlanAction::Research => run_research(flags, &mut streams),
        PlanAction::Tasks => run_tasks(flags, &mut streams),
        PlanAction::Check => run_check(flags, &mut streams),
    }
}

fn usage(standard_error: Output<'_>) -> u8 {
    let _ = writeln!(
        standard_error,
        "Usage: plan specify --request <text> | research --plan <id> [--claim <text> --source-url <url> --source-type <type> --retrieved-at <rfc3339> --support <text> --freshness <class> --used-by <ids>] | tasks --plan <id> | check [--rtm] --plan <id>"
    );
    1
}

fn action_flags(action: PlanAction) -> FlagSet {
    let mut flags = FlagSet::new(action.name());
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    match action {
        PlanAction::Specify => flags.string_flag("request", ""),
        PlanAction::Research => {
            flags.string_flag("plan", "");
            flags.string_flag("claim", "");
            flags.string_flag("source-url", "");
            flags.string_flag("source-type", "");
            flags.string_flag("publication-date", "");
            flags.string_flag("retrieved-at", "");
            flags.string_flag("support", "");
            flags.string_flag("freshness", "");
            flags.string_flag("used-by", "");
        }
        PlanAction::Tasks => flags.string_flag("plan", ""),
        PlanAction::Check => {
            flags.string_flag("plan", "");
            flags.bool_flag("rtm", false);
        }
    }
    flags
}

fn parse_action_flags(
    action: PlanAction,
    arguments: &[String],
    error_output: Output<'_>,
) -> Result<FlagSet, u8> {
    parse_flags(action_flags(action), arguments, error_output)
}

fn parse_flags(
    mut flags: FlagSet,
    arguments: &[String],
    error_output: Output<'_>,
) -> Result<FlagSet, u8> {
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(error_output, "{}", error.message);
        return Err(1);
    }
    if !flags.positional.is_empty() {
        let _ = writeln!(
            error_output,
            "{}: unexpected positional arguments: {}",
            flags.name,
            flags.positional.join(" ")
        );
        return Err(1);
    }
    Ok(flags)
}

fn planner_context(flags: &FlagSet) -> Result<PlannerContext, String> {
    let workspace_root = resolve_repository_root(flags.string_value("workspace-root"))?;
    if !workspace_root.is_dir() {
        return Err(format!(
            "plan: workspace root is not a directory: {}",
            display_path(&workspace_root)
        ));
    }
    let home = resolve_claude_home(flags.string_value("claude-home"))?;
    let workspace_key =
        crate::utility::system_map::workspace_key(&workspace_root.to_string_lossy());
    let plans_root = home
        .join("memories")
        .join("workspaces")
        .join(workspace_key)
        .join("plans");
    Ok(PlannerContext {
        home,
        workspace_root,
        plans_root,
    })
}

fn plan_paths(context: &PlannerContext, plan_id: &str) -> Result<PlanPaths, String> {
    let safe_id = safe_path_segment(plan_id).ok_or_else(|| {
        format!("invalid plan id {plan_id:?}: must be a single safe path segment")
    })?;
    Ok(PlanPaths::new(context.plans_root.join(safe_id)))
}

fn run_specify(flags: FlagSet, streams: &mut CommandStreams<'_>) -> u8 {
    let request = flags.string_value("request");
    if request.trim().is_empty() {
        let _ = writeln!(streams.error, "plan specify requires --request");
        return 1;
    }
    let context = command_or_return!(planner_context(&flags), streams.error);
    let plan_id = new_plan_id(request);
    let paths = command_or_return!(plan_paths(&context, &plan_id), streams.error);
    let vague_terms = vague_terms(request);
    let clarification_required = !vague_terms.is_empty();
    let created_at = timestamp();
    let artifacts = initial_artifacts(
        &plan_id,
        request,
        context.workspace(),
        &vague_terms,
        &created_at,
    );
    command_or_return!(
        write_new_bundle(&context.plans_root, &paths, &artifacts),
        streams.error
    );

    let payload = plan_payload(
        &plan_id,
        &paths,
        json!({
            "stage": "specified",
            "clarificationRequired": clarification_required,
        }),
    );
    emit_success(
        &flags,
        streams,
        &payload,
        &format!(
            "plan specify: id={} path={} status=specified{}",
            payload["planId"].as_str().unwrap_or_default(),
            display_path(&paths.directory),
            if clarification_required {
                " clarification=required"
            } else {
                ""
            }
        ),
    )
}

fn run_research(flags: FlagSet, streams: &mut CommandStreams<'_>) -> u8 {
    let (context, plan_id, paths) = parsed_or_return!(existing_plan(&flags, streams.error));
    let mut status = command_or_return!(load_status(&paths, &plan_id), streams.error);
    let request = string_field(&status, "request").unwrap_or_default();
    if request.trim().is_empty() {
        return command_error(streams.error, "status.json has no request");
    }
    let submitted = command_or_return!(submitted_research(&flags), streams.error);
    let bundle = command_or_return!(
        resolve_research_bundle(&context, request, submitted),
        streams.error
    );
    let research_status = if bundle.sources.is_empty() {
        "insufficient"
    } else {
        "complete"
    };
    let request_retrieved_at = timestamp();
    let mut claims = bundle.claims;
    claims.push(claim_record(
        "CLM-002",
        "The desired outcome is derived from the submitted user request.",
        "derived",
        vec!["SRC-REQUEST-001".to_string()],
    ));
    let research = versioned_value(
        &plan_id,
        Some("research"),
        json!({
            "status": research_status,
            "query": request,
            "truncated": bundle.truncated,
            "grounding": bundle.grounding,
            "researchSource": bundle.research_source,
            "sources": bundle.sources,
            "requestSource": request_source_record(&request_retrieved_at),
            "claims": claims
        }),
    );
    let architecture = render_architecture(
        &plan_id,
        request,
        bundle.primary_source.as_deref(),
        &bundle.grounding,
    );
    command_or_return!(
        write_json(&paths.research, &research)
            .and_then(|_| write_text(&paths.architecture, &architecture)),
        streams.error
    );
    let tasks_status = status_string(&status, "tasksStatus", "pending");
    let spec = command_or_return!(read_text(&paths.spec, SPEC_FILE), streams.error);
    let (parsed, mut research_issues) = validate_specification(&spec, &plan_id);
    validate_research(
        Some(&research),
        &parsed,
        context.workspace(),
        &mut research_issues,
    );
    let stage = if research_issues.is_empty() {
        "researched"
    } else {
        "specified"
    };
    let recorded_status = if research_issues.is_empty() {
        research_status.to_string()
    } else if research_status == "insufficient" {
        "insufficient".to_string()
    } else {
        "invalid".to_string()
    };
    update_status(
        &mut status,
        stage,
        recorded_status,
        tasks_status,
        "pending",
        research_issues.clone(),
    );
    command_or_return!(write_status(&paths, &status, "research"), streams.error);
    if research_status == "insufficient" {
        return command_error(
            streams.error,
            "plan research: no indexed local evidence matched the request; add grounded research before tasks",
        );
    }
    if !research_issues.is_empty() {
        return validation_errors("plan research", &research_issues, streams.error);
    }
    let mut payload = stage_payload(&plan_id, &paths, "researched");
    insert_string(&mut payload, "grounding", &bundle.grounding);
    insert_string(&mut payload, "researchSource", &bundle.research_source);
    emit_success(
        &flags,
        streams,
        &payload,
        &format!(
            "plan research: id={plan_id} status=researched sources={} grounding={} source={}",
            value_array(&research, "sources").map_or(0, <[Value]>::len),
            bundle.grounding,
            bundle.research_source
        ),
    )
}

fn run_tasks(flags: FlagSet, streams: &mut CommandStreams<'_>) -> u8 {
    let (context, plan_id, paths) = parsed_or_return!(existing_plan(&flags, streams.error));
    let spec = command_or_return!(read_text(&paths.spec, SPEC_FILE), streams.error);
    let (parsed, mut issues) = validate_specification(&spec, &plan_id);
    let research = load_json_artifact(&paths.research, RESEARCH_FILE, &plan_id, &mut issues);
    validate_research(research.as_ref(), &parsed, context.workspace(), &mut issues);
    if !issues.is_empty() {
        return validation_errors("plan tasks", &issues, streams.error);
    }
    let research = research.expect("research exists after validation");
    if string_field(&research, "status") != Some("complete") {
        return command_error(streams.error, "plan tasks: research status is not complete");
    }
    if spec.contains("Decision: unresolved_material_ambiguity") {
        return command_error(
            streams.error,
            "plan tasks: unresolved material ambiguity requires a recorded decision",
        );
    }

    let mut status = command_or_return!(load_status(&paths, &plan_id), streams.error);
    let (tasks, rtm) = build_tasks_and_rtm(&plan_id, &parsed);
    command_or_return!(
        write_json(&paths.tasks, &tasks).and_then(|_| write_json(&paths.rtm, &rtm)),
        streams.error
    );
    status
        .as_object_mut()
        .expect("validated status artifact is an object")
        .insert("clarificationRequired".to_string(), Value::Bool(false));
    update_status(
        &mut status,
        "tasked",
        "complete".to_string(),
        "ready".to_string(),
        "pending",
        Vec::new(),
    );
    command_or_return!(write_status(&paths, &status, "tasks"), streams.error);
    let payload = stage_payload(&plan_id, &paths, "tasked");
    emit_success(
        &flags,
        streams,
        &payload,
        &format!(
            "plan tasks: id={plan_id} status=tasked tasks={}",
            parsed.requirements.len()
        ),
    )
}

fn run_check(flags: FlagSet, streams: &mut CommandStreams<'_>) -> u8 {
    let (context, plan_id, paths) = parsed_or_return!(existing_plan(&flags, streams.error));

    let mut check_issues = Vec::new();
    let spec = read_text_for_validation(&paths.spec, SPEC_FILE, &mut check_issues);
    let (parsed, spec_issues) = spec
        .as_deref()
        .map(|body| validate_specification(body, &plan_id))
        .unwrap_or_default();
    check_issues.extend(spec_issues);
    let architecture =
        read_text_for_validation(&paths.architecture, ARCHITECTURE_FILE, &mut check_issues);
    if let Some(body) = architecture.as_deref() {
        validate_markdown_header(
            body,
            ARCHITECTURE_FILE,
            "architecture",
            &plan_id,
            &mut check_issues,
        );
    }
    let research = load_json_artifact(&paths.research, RESEARCH_FILE, &plan_id, &mut check_issues);
    let tasks = load_json_artifact(&paths.tasks, TASKS_FILE, &plan_id, &mut check_issues);
    let rtm = load_json_artifact(&paths.rtm, RTM_FILE, &plan_id, &mut check_issues);
    let status = load_json_artifact(&paths.status, STATUS_FILE, &plan_id, &mut check_issues);
    if let Some(body) = architecture.as_deref() {
        validate_architecture_claims(body, research.as_ref(), &mut check_issues);
    }
    validate_research(
        research.as_ref(),
        &parsed,
        context.workspace(),
        &mut check_issues,
    );
    validate_tasks(tasks.as_ref(), &parsed, &mut check_issues);
    validate_rtm(rtm.as_ref(), &parsed, &mut check_issues);
    validate_status(status.as_ref(), spec.as_deref(), &mut check_issues);

    if let Some(mut status) = status {
        if schema_is_current(&status) {
            let stage = if check_issues.is_empty() {
                "valid".to_string()
            } else {
                status_string(&status, "stage", "specified")
            };
            let check_status = if check_issues.is_empty() {
                "valid"
            } else {
                "invalid"
            };
            let research_status = status_string(&status, "researchStatus", "pending");
            let tasks_status = status_string(&status, "tasksStatus", "pending");
            update_status(
                &mut status,
                &stage,
                research_status,
                tasks_status,
                check_status,
                check_issues.clone(),
            );
            command_or_return!(write_status(&paths, &status, "check"), streams.error);
        }
    }

    if !check_issues.is_empty() {
        return validation_errors("plan check", &check_issues, streams.error);
    }
    let mut payload = plan_payload(
        &plan_id,
        &paths,
        json!({
            "status": "valid",
            "requirements": parsed.requirements.len(),
            "acceptanceCriteria": parsed.acceptance_criteria.len(),
        }),
    );
    if flags.bool_value("rtm") {
        payload
            .as_object_mut()
            .expect("check payload object")
            .insert("rtm".to_string(), rtm.expect("RTM exists after validation"));
    }
    emit_success(
        &flags,
        streams,
        &payload,
        &format!(
            "plan check: valid plan={plan_id} requirements={} acceptance_criteria={}",
            parsed.requirements.len(),
            parsed.acceptance_criteria.len()
        ),
    )
}

fn existing_plan(
    flags: &FlagSet,
    diagnostic_output: Output<'_>,
) -> Result<(PlannerContext, String, PlanPaths), u8> {
    let plan_id = flags.string_value("plan").trim();
    if plan_id.is_empty() {
        let _ = writeln!(diagnostic_output, "{} requires --plan", flags.name);
        return Err(1);
    }
    let context =
        planner_context(flags).map_err(|error| command_error(diagnostic_output, &error))?;
    let paths =
        plan_paths(&context, plan_id).map_err(|error| command_error(diagnostic_output, &error))?;
    if !paths.directory.is_dir() {
        let _ = writeln!(diagnostic_output, "plan not found: {plan_id}");
        return Err(1);
    }
    Ok((context, plan_id.to_string(), paths))
}

fn new_plan_id(request: &str) -> String {
    let fingerprint = crate::utility::hashing::fnv1a64_hex(request);
    format!(
        "plan-{}-{}-{}",
        Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        std::process::id(),
        &fingerprint[..8]
    )
}

fn submitted_research(flags: &FlagSet) -> Result<Option<SubmittedResearch>, String> {
    let names = [
        "claim",
        "source-url",
        "source-type",
        "publication-date",
        "retrieved-at",
        "support",
        "freshness",
        "used-by",
    ];
    if names
        .iter()
        .all(|name| flags.string_value(name).trim().is_empty())
    {
        return Ok(None);
    }
    let claim = flags.string_value("claim").trim();
    let source_url = flags.string_value("source-url").trim();
    let source_type = flags.string_value("source-type").trim();
    let retrieved_at = flags.string_value("retrieved-at").trim();
    let support = flags.string_value("support").trim();
    let freshness = flags.string_value("freshness").trim();
    let used_by_value = flags.string_value("used-by").trim();
    let required = |name: &str, value: &str| {
        if value.is_empty() {
            Err(format!("plan research external source requires --{name}"))
        } else {
            Ok(value.to_string())
        }
    };
    let publication_date = match flags.string_value("publication-date").trim() {
        "" => None,
        value => Some(value.to_string()),
    };
    let used_by = split_ids(&required("used-by", used_by_value)?);
    if used_by.is_empty() {
        return Err("plan research external source requires non-empty --used-by IDs".to_string());
    }
    Ok(Some(SubmittedResearch {
        claim: required("claim", claim)?,
        source_url: required("source-url", source_url)?,
        source_type: required("source-type", source_type)?,
        publication_date,
        retrieved_at: required("retrieved-at", retrieved_at)?,
        support: required("support", support)?,
        freshness: required("freshness", freshness)?,
        used_by,
    }))
}

fn resolve_research_bundle(
    context: &PlannerContext,
    request: &str,
    submitted: Option<SubmittedResearch>,
) -> Result<ResearchBundle, String> {
    if let Some(submitted) = submitted {
        return Ok(submitted_research_bundle(submitted));
    }
    let cached =
        crate::utility::memory_families::lookup_fresh_research_cache(&context.home, request)
            .map_err(|error| format!("plan research cache lookup: {error}"))?;
    if let Some(hit) = cached
        .fresh
        .into_iter()
        .min_by_key(research_cache_precedence)
    {
        return Ok(cached_research_bundle(hit));
    }
    if cached.stale_matches > 0 {
        return Err(format!(
            "plan research: {count} matching cache record(s) are stale; re-search required",
            count = cached.stale_matches
        ));
    }
    local_research_bundle(context, request)
}

fn research_cache_precedence(
    hit: &crate::utility::memory_families::ResearchCacheHit,
) -> (u8, std::cmp::Reverse<i64>) {
    let source_priority = match hit.source_type.as_str() {
        "official-doc" => 0,
        "repository" => 1,
        "issue" => 2,
        "standard" => 3,
        "paper" => 4,
        "local-code" => 5,
        "user-request" => 6,
        _ => 7,
    };
    let retrieved_at = chrono::DateTime::parse_from_rfc3339(&hit.retrieved_at)
        .map(|timestamp| timestamp.timestamp())
        .unwrap_or(i64::MIN);
    (source_priority, std::cmp::Reverse(retrieved_at))
}

fn submitted_research_bundle(submitted: SubmittedResearch) -> ResearchBundle {
    let source_id = "SRC-001".to_string();
    let publication_date = submitted
        .publication_date
        .map(Value::String)
        .unwrap_or(Value::Null);
    let source = json!({
        "sourceId": source_id,
        "sourceUrl": submitted.source_url,
        "sourceType": submitted.source_type,
        "publicationDate": publication_date,
        "retrievedAt": submitted.retrieved_at,
        "support": submitted.support,
        "freshness": submitted.freshness,
        "usedBy": submitted.used_by,
    });
    let claim = traceable_claim_record(
        "CLM-001",
        submitted.claim,
        "verified",
        vec![source_id],
        submitted.used_by,
    );
    ResearchBundle {
        primary_source: string_field(&source, "sourceUrl").map(str::to_string),
        grounding: string_field(&source, "freshness")
            .unwrap_or_default()
            .to_string(),
        research_source: "host".to_string(),
        sources: vec![source],
        claims: vec![claim],
        truncated: false,
    }
}

fn cached_research_bundle(
    hit: crate::utility::memory_families::ResearchCacheHit,
) -> ResearchBundle {
    let source_id = "SRC-CACHE-001".to_string();
    let publication_date = hit
        .publication_date
        .map(Value::String)
        .unwrap_or(Value::Null);
    let source = json!({
        "sourceId": source_id,
        "sourceUrl": hit.source_url,
        "sourceType": hit.source_type,
        "publicationDate": publication_date,
        "retrievedAt": hit.retrieved_at,
        "support": hit.answer,
        "freshness": hit.freshness_class,
        "usedBy": hit.used_by,
        "cacheId": hit.id,
    });
    let claim = traceable_claim_record(
        "CLM-001",
        string_field(&source, "support").unwrap_or_default(),
        "verified",
        vec![source_id],
        hit.used_by,
    );
    ResearchBundle {
        primary_source: string_field(&source, "sourceUrl").map(str::to_string),
        grounding: string_field(&source, "freshness")
            .unwrap_or_default()
            .to_string(),
        research_source: "cache".to_string(),
        sources: vec![source],
        claims: vec![claim],
        truncated: false,
    }
}

fn local_research_bundle(
    context: &PlannerContext,
    request: &str,
) -> Result<ResearchBundle, String> {
    let retrieved_at = timestamp();
    let search = crate::utility::workspace_index::search_with_metadata(
        context.workspace(),
        &context.home.to_string_lossy(),
        request,
        5,
    )
    .map_err(|error| format!("plan research: {error}"))?;
    let sources: Vec<Value> = search
        .hits
        .iter()
        .enumerate()
        .map(|(index, hit)| local_source_record(index, hit, &retrieved_at))
        .collect();
    let source_ids: Vec<String> = (1..=sources.len())
        .map(|index| format!("SRC-{index:03}"))
        .collect();
    let claim = claim_record(
        "CLM-001",
        if source_ids.is_empty() {
            "No indexed local evidence matched the submitted request."
        } else {
            "The workspace contains indexed local evidence relevant to the submitted request."
        },
        if source_ids.is_empty() {
            "assumption"
        } else {
            "verified"
        },
        source_ids,
    );
    Ok(ResearchBundle {
        primary_source: search.hits.first().map(|hit| hit.path.clone()),
        grounding: "local-only".to_string(),
        research_source: "local-index".to_string(),
        sources,
        claims: vec![claim],
        truncated: search.truncated,
    })
}

fn local_source_record(
    index: usize,
    hit: &crate::utility::workspace_index::SearchHit,
    retrieved_at: &str,
) -> Value {
    json!({
        "sourceId": format!("SRC-{:03}", index + 1),
        "sourceUrl": format!("local-code://{}#L{}-L{}", hit.path, hit.start_line, hit.end_line),
        "sourceType": "local-code",
        "publicationDate": Value::Null,
        "retrievedAt": retrieved_at,
        "support": hit.snippet,
        "freshness": "local-only",
        "searchReason": hit.reason,
        "score": hit.score,
        "usedBy": ["REQ-001", "AC-001"]
    })
}

fn request_source_record(retrieved_at: &str) -> Value {
    json!({
        "sourceId": "SRC-REQUEST-001",
        "sourceUrl": "request://submitted",
        "sourceType": "user-request",
        "publicationDate": Value::Null,
        "retrievedAt": retrieved_at,
        "support": "The submitted request is preserved verbatim in spec.md.",
        "freshness": "local-only",
        "usedBy": ["REQ-001", "AC-001"]
    })
}

fn claim_record(
    claim_id: &str,
    claim: impl Into<String>,
    classification: &str,
    source_ids: Vec<String>,
) -> Value {
    traceable_claim_record(
        claim_id,
        claim,
        classification,
        source_ids,
        vec!["REQ-001".to_string(), "AC-001".to_string()],
    )
}

fn traceable_claim_record(
    claim_id: &str,
    claim: impl Into<String>,
    classification: &str,
    source_ids: Vec<String>,
    used_by: Vec<String>,
) -> Value {
    json!({
        "claimId": claim_id,
        "claim": claim.into(),
        "classification": classification,
        "sourceIds": source_ids,
        "usedBy": used_by
    })
}

fn versioned_value(plan_id: &str, artifact: Option<&str>, body: Value) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("schemaVersion".to_string(), json!(SCHEMA_VERSION));
    if let Some(artifact) = artifact {
        object.insert("artifact".to_string(), Value::String(artifact.to_string()));
    }
    object.insert("planId".to_string(), Value::String(plan_id.to_string()));
    object.extend(
        body.as_object()
            .expect("versioned planner value body is an object")
            .clone(),
    );
    Value::Object(object)
}

fn plan_payload(plan_id: &str, paths: &PlanPaths, body: Value) -> Value {
    let mut payload = versioned_value(plan_id, None, body);
    payload
        .as_object_mut()
        .expect("planner command payload is an object")
        .insert(
            "planPath".to_string(),
            Value::String(display_path(&paths.directory)),
        );
    payload
}

fn insert_string(value: &mut Value, field_name: &str, field_value: &str) {
    value
        .as_object_mut()
        .expect("planner value is an object")
        .insert(
            field_name.to_string(),
            Value::String(field_value.to_string()),
        );
}

fn split_ids(value: &str) -> Vec<String> {
    value
        .split([',', ' '])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn vague_terms(request: &str) -> Vec<&'static str> {
    let normalized: String = request
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect();
    let words: BTreeSet<&str> = normalized.split_whitespace().collect();
    VAGUE_PREDICATES
        .into_iter()
        .filter(|predicate| words.contains(predicate))
        .collect()
}

fn initial_artifacts(
    plan_id: &str,
    request: &str,
    workspace_root: &Path,
    vague_terms: &[&str],
    created_at: &str,
) -> Vec<(&'static str, String)> {
    let clarification_required = !vague_terms.is_empty();
    let spec = render_specification(plan_id, request, vague_terms);
    let research = versioned_value(
        plan_id,
        Some("research"),
        json!({
            "status": "pending",
            "sources": [request_source_record(created_at)],
            "claims": [
                claim_record(
                    "CLM-001",
                    "The current workspace behavior still requires local research.",
                    "assumption",
                    Vec::new(),
                ),
                claim_record(
                    "CLM-002",
                    "The desired outcome is derived from the submitted user request.",
                    "derived",
                    vec!["SRC-REQUEST-001".to_string()],
                ),
            ]
        }),
    );
    let tasks = versioned_value(
        plan_id,
        Some("tasks"),
        json!({
            "status": "pending",
            "tasks": []
        }),
    );
    let rtm = versioned_value(
        plan_id,
        Some("rtm"),
        json!({
            "status": "pending",
            "entries": []
        }),
    );
    let status = versioned_value(
        plan_id,
        Some("status"),
        json!({
            "request": request,
            "workspaceRoot": display_path(workspace_root),
            "stage": "specified",
            "clarificationRequired": clarification_required,
            "researchStatus": "pending",
            "tasksStatus": "pending",
            "checkStatus": "pending",
            "errors": [],
            "updatedAt": created_at
        }),
    );
    vec![
        (SPEC_FILE, spec),
        (
            RESEARCH_FILE,
            render_json(&research).expect("serialize initial research"),
        ),
        (
            ARCHITECTURE_FILE,
            render_architecture(plan_id, request, None, "pending"),
        ),
        (
            TASKS_FILE,
            render_json(&tasks).expect("serialize initial tasks"),
        ),
        (RTM_FILE, render_json(&rtm).expect("serialize initial RTM")),
        (
            STATUS_FILE,
            render_json(&status).expect("serialize initial status"),
        ),
    ]
}

fn write_new_bundle(
    plans_root: &Path,
    paths: &PlanPaths,
    artifacts: &[(&str, String)],
) -> Result<(), String> {
    fs::create_dir_all(plans_root)
        .map_err(|error| format!("create {}: {error}", display_path(plans_root)))?;
    if paths.directory.exists() {
        return Err(format!(
            "plan already exists: {}",
            display_path(&paths.directory)
        ));
    }
    let staging = plans_root.join(format!(
        ".{}.tmp-{}",
        paths
            .directory
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("plan"),
        std::process::id()
    ));
    if staging.exists() {
        return Err(format!(
            "planner staging path already exists: {}",
            display_path(&staging)
        ));
    }
    fs::create_dir(&staging)
        .map_err(|error| format!("create {}: {error}", display_path(&staging)))?;
    let staged_result = artifacts
        .iter()
        .try_for_each(|(name, body)| write_text(&staging.join(name), body));
    if let Err(error) = staged_result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if let Err(error) = fs::rename(&staging, &paths.directory) {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!(
            "publish {}: {error}",
            display_path(&paths.directory)
        ));
    }
    Ok(())
}

fn render_specification(plan_id: &str, request: &str, vague_terms: &[&str]) -> String {
    let one_line_request = request.split_whitespace().collect::<Vec<_>>().join(" ");
    let verbatim_request = request
        .lines()
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let ambiguity = if vague_terms.is_empty() {
        "Decision: no_material_ambiguity_detected\n\nRationale: REQ-001 names an observable result and the AC supplies a verification result.\n\nSource IDs: SRC-REQUEST-001\n\nPlausible interpretations:\n- Implement the observable behavior exactly as stated in REQ-001.\n\nClarification question: none."
            .to_string()
    } else {
        format!(
            "Decision: unresolved_material_ambiguity\n\nRationale: The submitted terms do not define a unique observable success result.\n\nSource IDs: SRC-REQUEST-001\n\nDetected terms: {}\n\nPlausible interpretations:\n- Define a measurable latency or throughput threshold.\n- Define the security property, threat boundary, and required failure behavior.\n- Limit the request to a named component and observable result.\n\nClarification question: Which interpretation and measurable threshold define success?",
            vague_terms.join(", ")
        )
    };
    format!(
        "---\nschema_version: 1\nartifact: spec\nplan_id: {plan_id}\n---\n\n# Plan Specification\n\n## 1. User request verbatim\n\n{verbatim_request}\n\n## 2. Restated user outcome\n\n[derived: CLM-002] Deliver the submitted outcome as an observable, reversible change: {one_line_request}\n\n## 3. Problem statement\n\n[derived: CLM-002] The submitted outcome does not yet have a compiled delivery contract linking requirements, evidence, and owners.\n\n## 4. Scope\n\n- Implement only the behavior stated in REQ-001.\n- Preserve the submitted request without reinterpretation.\n\n## 5. Non-goals\n\n- Features or refactors not required by REQ-001.\n- Silent resolution of material ambiguity.\n\n## 6. Constraints\n\n- Preserve existing data and behavior outside the named scope.\n- Use observable verification evidence.\n\n## 7. Stakeholders/users affected\n\n- Requesting user\n- Implementer\n- Verifier\n- Reviewer\n\n## 8. Current behavior\n\n[assumption: CLM-001] Current workspace behavior must be verified by `keel plan research` before tasks are compiled.\n\n## 9. Desired behavior\n\n[derived: CLM-002] The observable result in REQ-001 is delivered without changing non-goals.\n\n## 10. Functional requirements\n\n### REQ-001\n\nID: REQ-001\n\nStatement: {one_line_request}\n\nClaim classification: derived\n\nSource IDs: SRC-REQUEST-001\n\n## 11. Non-functional requirements\n\n- NFR-001: Verification output must identify the command or inspection used and its observable result.\n- NFR-002: Rollback must preserve pre-existing data.\n\n## 12. Acceptance criteria\n\n### AC-001 - Submitted outcome is observable\n\nID: AC-001\n\nRequirement references: REQ-001\n\nPrecondition: The workspace contains the completed REQ-001 change.\n\nAction: Run the generated task verification command and inspect the named output.\n\nExpected observable outcome: The result stated by REQ-001 is present and the verification command exits 0.\n\nNegative/failure outcome: A non-zero exit or missing named result fails the criterion.\n\nVerification method: Run the generated task verification command and inspect its exit status.\n\nExpected evidence type: Command capture with exit code and observable output.\n\nOwner role: verifier\n\n## 13. Edge cases\n\n- Empty, malformed, or unsupported artifact schemas fail closed.\n- Missing requirement mappings and evidence methods fail closed.\n- Material ambiguity remains visible until a decision is recorded.\n\n## 14. Failure behavior\n\nValidation emits every actionable defect and returns a non-zero exit code.\n\n## 15. Security/privacy concerns\n\nDo not record credentials, tokens, or unrelated user data in plan artifacts.\n\n## 16. Performance and token constraints\n\nUse bounded artifacts and keep the fixed-context planner pointer within its ratified token budget.\n\n## 17. Compatibility/host constraints\n\nArtifacts use portable UTF-8 text and JSON under the canonical host-neutral Keel memory lane.\n\n## 18. Rollout and rollback requirements\n\nRoll out only after `keel plan check` passes. Revert the implementation commit to roll back code; retain plan evidence.\n\n## 19. Assumptions\n\n- ASM-001 [assumption]: Local source research must complete before implementation.\n\n## 20. Ambiguities and clarification decisions\n\n{ambiguity}\n\n## 21. Research references\n\n- CLM-001: workspace behavior claim; classification stored in research.json.\n- CLM-002: user-outcome derivation; classification stored in research.json.\n\n## 22. Verification strategy\n\nRun every AC verification method, preserve its expected evidence type, run `keel plan check --rtm --plan {plan_id}`, and require reviewer confirmation before delivery.\n"
    )
}

fn render_architecture(
    plan_id: &str,
    request: &str,
    source: Option<&str>,
    grounding: &str,
) -> String {
    let current = match source {
        Some(source) => {
            format!("[verified: CLM-001] Primary {grounding} source anchor `{source}` was read.")
        }
        None => "[assumption: CLM-001] Source research is pending.".to_string(),
    };
    let outcome = request.split_whitespace().collect::<Vec<_>>().join(" ");
    format!(
        "---\nschema_version: 1\nartifact: architecture\nplan_id: {plan_id}\n---\n\n# Architecture Note\n\n## 1. Current architecture relevant to scope\n\n{current}\n\n## 2. Proposed architecture\n\n[derived: CLM-002] Implement only the smallest owner path needed for: {outcome}\n\n## 3. Components/files/interfaces changed\n\nPending implementer source trace.\n\n## 4. Data/control flow\n\nPending implementer source trace.\n\n## 5. Alternatives considered\n\nNo implementation alternative is selected until source research completes.\n\n## 6. Why the chosen option fits requirements\n\nThe chosen option must map to REQ-001 and AC-001.\n\n## 7. Risks and mitigations\n\nRisk: scope drift. Mitigation: validate the RTM before implementation.\n\n## 8. Backward compatibility\n\nPreserve behavior outside REQ-001.\n\n## 9. Error handling and fallback semantics\n\nMaterial failures are explicit; no silent fallback is authorized.\n\n## 10. Security/privacy implications\n\nDo not persist credentials or unrelated user data.\n\n## 11. Performance/token impact\n\nUse bounded artifacts and the ratified planner pointer.\n\n## 12. Test strategy\n\nExecute the AC-001 verification method and the repository release ladder.\n\n## 13. Rollback strategy\n\nRevert the implementation while retaining planning evidence.\n\n## 14. Requirement and research references\n\nRequirement references: REQ-001\n\nClaim references: CLM-001, CLM-002\n"
    )
}

fn build_tasks_and_rtm(plan_id: &str, parsed: &ParsedSpecification) -> (Value, Value) {
    let mut task_entries = Vec::new();
    let mut rtm_entries = Vec::new();
    for (index, requirement) in parsed.requirements.iter().enumerate() {
        let mapped: Vec<&AcceptanceCriterion> = parsed
            .acceptance_criteria
            .iter()
            .filter(|criterion| criterion.requirement_ids.contains(&requirement.id))
            .collect();
        let task_id = format!("TASK-{:03}", index + 1);
        let criterion_ids: Vec<&str> = mapped
            .iter()
            .map(|criterion| criterion.id.as_str())
            .collect();
        let verification_methods: Vec<&str> = mapped
            .iter()
            .map(|criterion| criterion.verification_method.as_str())
            .collect();
        let evidence_types: Vec<&str> = mapped
            .iter()
            .map(|criterion| criterion.evidence_type.as_str())
            .collect();
        task_entries.push(json!({
            "taskId": task_id,
            "title": format!("Implement {}", requirement.id),
            "requirementIds": [requirement.id],
            "acceptanceCriterionIds": criterion_ids,
            "verificationMethod": verification_methods.first().copied().unwrap_or_default(),
            "expectedEvidenceType": evidence_types.first().copied().unwrap_or_default(),
            "ownerRole": "implementer",
            "status": "pending"
        }));
        rtm_entries.push(json!({
            "requirementId": requirement.id,
            "acceptanceCriterionIds": criterion_ids,
            "taskIds": [task_id],
            "verificationMethods": verification_methods,
            "expectedEvidenceTypes": evidence_types
        }));
    }
    (
        versioned_value(
            plan_id,
            Some("tasks"),
            json!({
                "status": "ready",
                "tasks": task_entries
            }),
        ),
        versioned_value(
            plan_id,
            Some("rtm"),
            json!({
                "status": "complete",
                "entries": rtm_entries
            }),
        ),
    )
}

fn validate_specification(spec: &str, plan_id: &str) -> (ParsedSpecification, Vec<String>) {
    let mut specification_issues = Vec::new();
    validate_markdown_header(spec, SPEC_FILE, "spec", plan_id, &mut specification_issues);
    for (index, section) in SPEC_SECTIONS.iter().enumerate() {
        let heading = format!("## {}. {section}", index + 1);
        if !spec.contains(&heading) {
            specification_issues.push(format!("spec.md missing section {heading}"));
        }
    }
    if spec.contains("Decision: unresolved_material_ambiguity") {
        specification_issues
            .push("unresolved material ambiguity requires a recorded decision".to_string());
    }

    let parsed = parse_specification(spec);
    if parsed.requirements.is_empty() {
        specification_issues.push("spec.md has no functional requirements".to_string());
    }
    if parsed.acceptance_criteria.is_empty() {
        specification_issues.push("spec.md has no acceptance criteria".to_string());
    }
    let requirement_ids: BTreeSet<&str> = parsed
        .requirements
        .iter()
        .map(|requirement| requirement.id.as_str())
        .collect();
    if requirement_ids.len() != parsed.requirements.len() {
        specification_issues.push("spec.md contains duplicate requirement IDs".to_string());
    }
    let criterion_ids: BTreeSet<&str> = parsed
        .acceptance_criteria
        .iter()
        .map(|criterion| criterion.id.as_str())
        .collect();
    if criterion_ids.len() != parsed.acceptance_criteria.len() {
        specification_issues
            .push("spec.md contains duplicate acceptance criterion IDs".to_string());
    }
    for requirement in &parsed.requirements {
        if !matches!(
            requirement.classification.as_str(),
            "verified" | "assumption" | "derived"
        ) {
            specification_issues.push(format!(
                "{} has unclassified factual claim; expected verified, assumption, or derived",
                requirement.id
            ));
        }
        if !parsed
            .acceptance_criteria
            .iter()
            .any(|criterion| criterion.requirement_ids.contains(&requirement.id))
        {
            specification_issues.push(format!("{} has no acceptance criterion", requirement.id));
        }
    }
    for criterion in &parsed.acceptance_criteria {
        validate_acceptance_criterion(criterion, &requirement_ids, &mut specification_issues);
    }
    (parsed, specification_issues)
}

fn parse_specification(spec: &str) -> ParsedSpecification {
    let lines: Vec<&str> = spec.lines().collect();
    let mut parsed = ParsedSpecification::default();
    for (index, line) in lines.iter().enumerate() {
        let Some(id) = heading_id(line) else {
            continue;
        };
        let end = lines[index + 1..]
            .iter()
            .position(|candidate| candidate.starts_with("### ") || candidate.starts_with("## "))
            .map(|offset| index + 1 + offset)
            .unwrap_or(lines.len());
        let block = &lines[index + 1..end];
        if id.starts_with("REQ-") {
            parsed.requirements.push(Requirement {
                id,
                classification: field(block, "Claim classification:"),
            });
        } else if id.starts_with("AC-") {
            parsed.acceptance_criteria.push(AcceptanceCriterion {
                id,
                requirement_ids: list_field(block, "Requirement references:"),
                precondition: field(block, "Precondition:"),
                action: field(block, "Action:"),
                expected_outcome: field(block, "Expected observable outcome:"),
                negative_outcome: field(block, "Negative/failure outcome:"),
                verification_method: field(block, "Verification method:"),
                evidence_type: field(block, "Expected evidence type:"),
                owner_role: field(block, "Owner role:"),
            });
        }
    }
    parsed
}

fn heading_id(line: &str) -> Option<String> {
    let heading = line.strip_prefix("### ")?;
    let id = heading.split_whitespace().next()?.trim();
    if id.starts_with("REQ-") || id.starts_with("AC-") {
        Some(id.to_string())
    } else {
        None
    }
}

fn field(block: &[&str], prefix: &str) -> String {
    block
        .iter()
        .find_map(|line| line.trim().strip_prefix(prefix).map(str::trim))
        .unwrap_or_default()
        .to_string()
}

fn list_field(block: &[&str], prefix: &str) -> Vec<String> {
    field(block, prefix)
        .split([',', ' '])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn validate_acceptance_criterion(
    criterion: &AcceptanceCriterion,
    requirement_ids: &BTreeSet<&str>,
    issues: &mut Issues,
) {
    if criterion.requirement_ids.is_empty() {
        issues.push(format!("{} has no requirement references", criterion.id));
    }
    for requirement_id in &criterion.requirement_ids {
        if !requirement_ids.contains(requirement_id.as_str()) {
            issues.push(format!(
                "{} references unknown requirement {requirement_id}",
                criterion.id
            ));
        }
    }
    for (label, value) in [
        ("precondition", &criterion.precondition),
        ("action", &criterion.action),
        ("expected observable outcome", &criterion.expected_outcome),
        ("verification method", &criterion.verification_method),
        ("expected evidence type", &criterion.evidence_type),
    ] {
        if value.trim().is_empty() {
            issues.push(format!("{} has no {label}", criterion.id));
        }
    }
    if criterion.negative_outcome.trim().is_empty() {
        issues.push(format!(
            "{} has no negative/failure outcome or explicit not-applicable rationale",
            criterion.id
        ));
    }
    if !matches!(
        criterion.owner_role.as_str(),
        "implementer" | "verifier" | "reviewer" | "human"
    ) {
        issues.push(format!("{} has invalid owner role", criterion.id));
    }
    let observable_text = format!("{} {}", criterion.action, criterion.expected_outcome);
    if contains_vague_predicate(&observable_text) && !has_observable_threshold(&observable_text) {
        issues.push(format!(
            "{} uses a vague predicate without an observable threshold",
            criterion.id
        ));
    }
}

fn contains_vague_predicate(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
    VAGUE_PREDICATES.iter().any(|predicate| {
        normalized
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|word| word == *predicate)
    })
}

fn has_observable_threshold(text: &str) -> bool {
    text.chars().any(|character| character.is_ascii_digit())
        || [
            "exits zero",
            "exit 0",
            "zero diagnostics",
            "at most",
            "under ",
            "measured",
        ]
        .iter()
        .any(|marker| text.to_ascii_lowercase().contains(marker))
}

fn validate_markdown_header(
    body: &str,
    file_name: &str,
    artifact: &str,
    plan_id: &str,
    issues: &mut Issues,
) {
    let expected = [
        "---".to_string(),
        "schema_version: 1".to_string(),
        format!("artifact: {artifact}"),
        format!("plan_id: {plan_id}"),
        "---".to_string(),
    ];
    let actual: Vec<&str> = body.lines().take(expected.len()).collect();
    if actual.len() != expected.len()
        || actual
            .iter()
            .zip(expected.iter())
            .any(|(actual, expected)| actual.trim() != expected)
    {
        issues.push(format!(
            "{file_name} has invalid schema header; expected schema_version 1, artifact {artifact}, plan_id {plan_id}"
        ));
    }
}

fn validate_research(
    research: Option<&Value>,
    parsed: &ParsedSpecification,
    workspace_root: &Path,
    issues: &mut Issues,
) {
    let Some(research) = research else {
        return;
    };
    let valid_uses: BTreeSet<&str> = parsed
        .requirements
        .iter()
        .map(|requirement| requirement.id.as_str())
        .chain(
            parsed
                .acceptance_criteria
                .iter()
                .map(|criterion| criterion.id.as_str()),
        )
        .collect();
    let policy = match crate::utility::research_policy::ResearchPolicy::load(workspace_root) {
        Ok(policy) => policy,
        Err(error) => {
            issues.push(error);
            return;
        }
    };
    issues.extend(crate::utility::research_policy::validate_research_artifact(
        research,
        &valid_uses,
        policy,
        Utc::now(),
    ));
}

pub(crate) fn review_research_issues(
    workspace_root: &Path,
    claude_home: &str,
    plan_id: &str,
) -> Result<Vec<String>, String> {
    let safe_id = safe_path_segment(plan_id).ok_or_else(|| {
        format!("invalid plan id {plan_id:?}: must be a single safe path segment")
    })?;
    let home = resolve_claude_home(claude_home)?;
    let workspace_key =
        crate::utility::system_map::workspace_key(&workspace_root.to_string_lossy());
    let plan_directory = home
        .join("memories")
        .join("workspaces")
        .join(workspace_key)
        .join("plans")
        .join(safe_id);
    if !plan_directory.is_dir() {
        return Err(format!("plan not found: {plan_id}"));
    }
    let paths = PlanPaths::new(plan_directory);
    let spec = read_text(&paths.spec, SPEC_FILE)?;
    let (parsed, mut issues) = validate_specification(&spec, plan_id);
    let research = load_json_artifact(&paths.research, RESEARCH_FILE, plan_id, &mut issues);
    validate_research(research.as_ref(), &parsed, workspace_root, &mut issues);
    Ok(issues)
}

fn validate_architecture_claims(architecture: &str, research: Option<&Value>, issues: &mut Issues) {
    let references = architecture
        .lines()
        .find_map(|line| line.trim().strip_prefix("Claim references:"))
        .map(|value| {
            value
                .split([',', ' '])
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if references.is_empty() {
        issues.push("architecture.md has no claim references".to_string());
        return;
    }
    let research_ids: BTreeSet<&str> = research
        .and_then(|value| value_array(value, "claims"))
        .into_iter()
        .flatten()
        .filter_map(|claim| string_field(claim, "claimId"))
        .collect();
    for claim_id in references {
        if !["verified", "assumption", "derived"]
            .iter()
            .any(|classification| architecture.contains(&format!("[{classification}: {claim_id}]")))
        {
            issues.push(format!("architecture.md claim {claim_id} is unclassified"));
        }
        if !research_ids.contains(claim_id) {
            issues.push(format!(
                "architecture.md references unknown research claim {claim_id}"
            ));
        }
    }
}

fn validate_tasks(tasks: Option<&Value>, parsed: &ParsedSpecification, issues: &mut Issues) {
    let Some(tasks) = tasks else {
        return;
    };
    if string_field(tasks, "status") != Some("ready") {
        issues.push("tasks.json status is not ready".to_string());
    }
    let task_entries = value_array(tasks, "tasks").unwrap_or_default();
    for requirement in &parsed.requirements {
        if !task_entries.iter().any(|task| {
            string_array(task, "requirementIds")
                .iter()
                .any(|id| id == &requirement.id)
        }) {
            issues.push(format!("{} has no implementation task", requirement.id));
        }
    }
    for criterion in &parsed.acceptance_criteria {
        if !task_entries.iter().any(|task| {
            string_array(task, "acceptanceCriterionIds")
                .iter()
                .any(|id| id == &criterion.id)
        }) {
            issues.push(format!("{} has no task mapping", criterion.id));
        }
    }
    for task in task_entries {
        let id = string_field(task, "taskId").unwrap_or("task without id");
        if string_field(task, "verificationMethod")
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            issues.push(format!("{id} has no verification method"));
        }
        if string_field(task, "expectedEvidenceType")
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            issues.push(format!("{id} has no expected evidence type"));
        }
    }
}

fn validate_rtm(rtm: Option<&Value>, parsed: &ParsedSpecification, issues: &mut Issues) {
    let Some(rtm) = rtm else {
        return;
    };
    if string_field(rtm, "status") != Some("complete") {
        issues.push("rtm.json status is not complete".to_string());
    }
    let entries = value_array(rtm, "entries").unwrap_or_default();
    for requirement in &parsed.requirements {
        let Some(entry) = entries
            .iter()
            .find(|entry| string_field(entry, "requirementId") == Some(requirement.id.as_str()))
        else {
            issues.push(format!("RTM missing {}", requirement.id));
            continue;
        };
        if string_array(entry, "acceptanceCriterionIds").is_empty() {
            issues.push(format!("RTM {} has no acceptance criteria", requirement.id));
        }
        if string_array(entry, "taskIds").is_empty() {
            issues.push(format!("RTM {} has no tasks", requirement.id));
        }
        if string_array(entry, "verificationMethods").is_empty() {
            issues.push(format!(
                "RTM {} has no verification methods",
                requirement.id
            ));
        }
        if string_array(entry, "expectedEvidenceTypes").is_empty() {
            issues.push(format!("RTM {} has no evidence types", requirement.id));
        }
    }
}

fn validate_status(status: Option<&Value>, spec: Option<&str>, issues: &mut Issues) {
    let Some(status) = status else {
        return;
    };
    if string_field(status, "researchStatus") != Some("complete") {
        issues.push("status.json researchStatus is not complete".to_string());
    }
    if string_field(status, "tasksStatus") != Some("ready") {
        issues.push("status.json tasksStatus is not ready".to_string());
    }
    if status.get("clarificationRequired").and_then(Value::as_bool) == Some(true)
        && spec
            .map(|body| body.contains("Decision: unresolved_material_ambiguity"))
            .unwrap_or(true)
    {
        issues.push("status.json records unresolved material ambiguity".to_string());
    }
}

fn load_json_artifact(
    path: &Path,
    file_name: &str,
    plan_id: &str,
    issues: &mut Issues,
) -> Option<Value> {
    let body = match fs::read_to_string(path) {
        Ok(body) => body,
        Err(error) => {
            issues.push(format!("read {file_name}: {error}"));
            return None;
        }
    };
    let value: Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(error) => {
            issues.push(format!("parse {file_name}: {error}"));
            return None;
        }
    };
    match value.get("schemaVersion").and_then(Value::as_u64) {
        Some(version) if version == SCHEMA_VERSION => {}
        Some(version) => issues.push(format!(
            "{file_name} schemaVersion={version}; expected {SCHEMA_VERSION}"
        )),
        None => issues.push(format!("{file_name} has no numeric schemaVersion")),
    }
    if string_field(&value, "planId") != Some(plan_id) {
        issues.push(format!("{file_name} planId does not match {plan_id}"));
    }
    let expected_artifact = file_name.trim_end_matches(".json");
    if string_field(&value, "artifact") != Some(expected_artifact) {
        issues.push(format!(
            "{file_name} artifact does not match {expected_artifact}"
        ));
    }
    Some(value)
}

fn load_status(paths: &PlanPaths, plan_id: &str) -> Result<Value, String> {
    let mut status_issues = Vec::new();
    let status = load_json_artifact(&paths.status, STATUS_FILE, plan_id, &mut status_issues);
    if status_issues.is_empty() {
        status.ok_or_else(|| "status.json is unavailable".to_string())
    } else {
        Err(status_issues.join("; "))
    }
}

fn read_text(path: &Path, file_name: &str) -> Result<String, String> {
    fs::read_to_string(path).map_err(|error| format!("read {file_name}: {error}"))
}

fn read_text_for_validation(path: &Path, file_name: &str, issues: &mut Issues) -> Option<String> {
    match read_text(path, file_name) {
        Ok(body) => Some(body),
        Err(error) => {
            issues.push(error);
            None
        }
    }
}

fn string_array(value: &Value, field_name: &str) -> Vec<String> {
    value_array(value, field_name)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn value_array<'a>(value: &'a Value, field_name: &str) -> Option<&'a [Value]> {
    value.get(field_name)?.as_array().map(Vec::as_slice)
}

fn string_field<'a>(value: &'a Value, field_name: &str) -> Option<&'a str> {
    value.get(field_name)?.as_str()
}

fn update_status(
    status: &mut Value,
    stage: &str,
    research_status: String,
    tasks_status: String,
    check_status: &str,
    errors: Vec<String>,
) {
    let object = status
        .as_object_mut()
        .expect("validated status artifact is an object");
    object.insert("stage".to_string(), Value::String(stage.to_string()));
    object.insert("researchStatus".to_string(), Value::String(research_status));
    object.insert("tasksStatus".to_string(), Value::String(tasks_status));
    object.insert(
        "checkStatus".to_string(),
        Value::String(check_status.to_string()),
    );
    object.insert(
        "errors".to_string(),
        Value::Array(errors.into_iter().map(Value::String).collect()),
    );
    object.insert("updatedAt".to_string(), Value::String(timestamp()));
}

fn status_string(status: &Value, field_name: &str, fallback: &str) -> String {
    string_field(status, field_name)
        .unwrap_or(fallback)
        .to_string()
}

fn schema_is_current(value: &Value) -> bool {
    value.get("schemaVersion").and_then(Value::as_u64) == Some(SCHEMA_VERSION)
}

fn stage_payload(plan_id: &str, paths: &PlanPaths, stage: &str) -> Value {
    plan_payload(
        plan_id,
        paths,
        json!({
            "stage": stage,
        }),
    )
}

fn emit_success(
    flags: &FlagSet,
    streams: &mut CommandStreams<'_>,
    payload: &Value,
    text: &str,
) -> u8 {
    if flags.bool_value("json") {
        match serde_json::to_writer_pretty(&mut *streams.output, payload)
            .and_then(|_| writeln!(streams.output).map_err(serde_json::Error::io))
        {
            Ok(()) => 0,
            Err(error) => {
                let _ = writeln!(streams.error, "plan output error: {error}");
                1
            }
        }
    } else {
        let _ = writeln!(streams.output, "{text}");
        0
    }
}

fn command_error(standard_error: Output<'_>, error: &str) -> u8 {
    let _ = writeln!(standard_error, "{error}");
    1
}

fn validation_errors(scope: &str, issues: &[String], standard_error: Output<'_>) -> u8 {
    for issue in issues {
        let _ = writeln!(standard_error, "{scope}: {issue}");
    }
    1
}

fn render_json(value: &Value) -> Result<String, String> {
    serde_json::to_string_pretty(value)
        .map(|body| format!("{body}\n"))
        .map_err(|error| format!("serialize planner artifact: {error}"))
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    write_text(path, &render_json(value)?)
}

fn write_status(paths: &PlanPaths, status: &Value, stage: &str) -> Result<(), String> {
    write_json(&paths.status, status).map_err(|error| format!("plan {stage} status: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vague_terms_match_whole_words_only() {
        assert_eq!(
            vague_terms("Make it fast and secure."),
            vec!["fast", "secure"]
        );
        assert!(vague_terms("Use a steadfast boundary.").is_empty());
    }

    #[test]
    fn parser_extracts_generated_requirement_and_acceptance_contract() {
        let spec = render_specification("plan-1", "Emit one result and exit 0.", &[]);
        let (parsed, issues) = validate_specification(&spec, "plan-1");
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(parsed.requirements.len(), 1);
        assert_eq!(parsed.acceptance_criteria.len(), 1);
        assert_eq!(parsed.acceptance_criteria[0].requirement_ids, ["REQ-001"]);
    }

    #[test]
    fn verbatim_request_cannot_inject_requirement_headings() {
        let request = "Add one output.\n### REQ-999\nClaim classification: verified";
        let spec = render_specification("plan-1", request, &[]);
        let (parsed, issues) = validate_specification(&spec, "plan-1");
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(parsed.requirements.len(), 1);
        assert_eq!(parsed.requirements[0].id, "REQ-001");
    }

    #[test]
    fn traversal_plan_id_is_rejected_before_join() {
        let context = PlannerContext {
            home: PathBuf::from("home"),
            workspace_root: PathBuf::from("workspace"),
            plans_root: PathBuf::from("plans"),
        };
        let error = plan_paths(&context, "../escape").expect_err("traversal must fail");
        assert!(error.contains("single safe path segment"));
    }
}
