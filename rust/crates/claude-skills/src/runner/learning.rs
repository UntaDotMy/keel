//! Purpose: The autonomous learning loop — observe -> instinct -> generated skill.
//!   Distills repeated behavioral observations into confidence-scored instincts,
//!   then evolves trusted instinct clusters into generated SKILL.md artifacts
//!   (each with a preloaded subagent), entirely without a manual slash command.
//! Caller: runner::hook_lifecycle on SessionEnd (automatic); runner::run_learn_command
//!   (manual inspection / dry-run).
//! Dependencies: runner::observation (signal source), utility::record_store
//!   (instinct persistence), runtime path helpers.
//! Main Functions: run_learning_cycle, CycleReport.
//! Side Effects: Writes instinct records under `<claude_home>/memory/instincts/`,
//!   generated skills under `<claude_home>/skills/learned-*`, and generated
//!   subagents under `<claude_home>/agents/learned-*.md`.
//!
//! Provenance is the spine of this module. Every artifact it writes is marked
//! three ways — a `learned-` name prefix, a `generated: true` frontmatter flag,
//! and a `.learning-meta.json` sidecar. The loop ONLY ever rewrites artifacts it
//! generated, NEVER a built-in skill synced from the repository. If a generated
//! skill's on-disk content no longer matches the hash we last wrote, the loop
//! treats it as a deliberate manual/agent refinement and leaves it untouched —
//! the "edit over create, never clobber" discipline. This is what lets the
//! answer to "should the agent be able to rewrite generated skills?" be "yes"
//! without the loop fighting the agent.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::args::FlagSet;
use crate::runner::observation::{self, Observation};
use crate::runtime::{agents_directory, display_path, resolve_claude_home, skills_directory};
use crate::utility::record_store::{field, Record, RecordStore};

/// Rolling window of observation history the cycle distills each run. Older
/// signal naturally ages out of the window, giving confidence an implicit decay
/// without a separate decay pass: a habit the user dropped stops being counted.
const OBSERVE_WINDOW_DAYS: u64 = 7;

/// Minimum observations of a signature within the window before it is worth
/// recording as an instinct at all. Below this it is a one-off, not a pattern.
const INSTINCT_MIN_COUNT: u64 = 3;

/// Confidence ceiling so a single hyperactive day cannot dominate. Confidence is
/// an integer (consistent with the manual `memory instincts` store) equal to the
/// windowed observation count, clamped here.
const INSTINCT_CONFIDENCE_CAP: i64 = 20;

/// An instinct must reach this confidence AND have been seen across more than one
/// session before it can contribute to a generated skill. Cross-session recurrence
/// is what separates a durable habit from a single long sitting.
const SKILL_MIN_CONFIDENCE: i64 = 5;

/// A project needs at least this many trusted instincts before a skill is worth
/// generating — one lone instinct does not justify a whole skill file.
const SKILL_MIN_INSTINCTS: usize = 2;

/// Auto-learned instincts whose confidence has decayed to this floor (their
/// pattern has aged out of every observation window) are pruned so the store
/// reflects current behavior rather than an ever-growing archive. Manual
/// instincts (`source != observed`) are never pruned regardless of confidence.
const INSTINCT_PRUNE_FLOOR: i64 = 0;

/// Group path for the shared instinct store, matching the manual
/// `claude-skills memory instincts` surface so auto-learned and hand-authored
/// instincts live in one place. The loop only ever manages records it marks
/// `source = observed`; manual instincts are never rewritten.
const INSTINCT_GROUP: &str = "memory/instincts";

/// Marker filename written inside every generated skill directory. Its presence
/// identifies a directory as loop-generated (vs a built-in synced skill) and its
/// stored hash powers the no-clobber guard.
const LEARNING_META_FILE: &str = ".learning-meta.json";

/// Field value marking an instinct (and skill/agent) as loop-generated.
const SOURCE_OBSERVED: &str = "observed";

/// Options controlling one cycle run. Defaults match the constants above; the
/// CLI inspection surface flips `dry_run` to preview without writing.
pub struct CycleOptions {
    /// When true, compute everything but write nothing to disk.
    pub dry_run: bool,
    /// Observation window in days. `0` reads nothing (used by `--window 0`).
    pub window_days: u64,
    /// When true, collect a per-skill synthesis brief for every generated skill
    /// still at its deterministic-template state. The brief is a ready-to-use
    /// instruction the session agent fulfils to replace the template prose with
    /// richer, LLM-authored prose — the binary never calls an LLM itself.
    pub synthesize: bool,
}

impl Default for CycleOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            window_days: OBSERVE_WINDOW_DAYS,
            synthesize: false,
        }
    }
}

/// A ready-to-use instruction for the session agent to rewrite one generated
/// skill's prose. Emitted by `learn synthesize`; the agent's resulting edit is
/// protected from the next cycle by the content-hash no-clobber guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthesisBrief {
    pub skill_name: String,
    pub skill_path: String,
    pub project: String,
    pub prompt: String,
}

/// What one cycle did, for logging and the `learn` inspection surface.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CycleReport {
    /// Distinct (project, signature) instincts recorded or refreshed.
    pub instincts_recorded: usize,
    /// Skills generated or regenerated this cycle.
    pub skills_generated: usize,
    /// Skills left untouched because their content was manually refined.
    pub skills_respected: usize,
    /// Subagents generated to preload a new skill.
    pub agents_generated: usize,
    /// Auto-learned instincts decayed and removed because their pattern aged out.
    pub instincts_pruned: usize,
    /// Human-readable per-skill notes (skill name -> what happened).
    pub notes: Vec<String>,
    /// Synthesis briefs for generated skills still at their template state.
    /// Populated only when `CycleOptions.synthesize` is set.
    pub synthesis_briefs: Vec<SynthesisBrief>,
}

/// Run one full learning cycle against `claude_home`.
///
/// Fail-open by contract: every fallible step logs to `log` and continues, so a
/// learning failure can never break the SessionEnd hook that calls it. Returns a
/// `CycleReport` describing what changed.
pub fn run_learning_cycle(
    claude_home: &Path,
    options: &CycleOptions,
    log: &mut dyn std::io::Write,
) -> CycleReport {
    let mut report = CycleReport::default();

    let observations = match observation::iter_recent_rows(options.window_days) {
        Ok(rows) => rows,
        Err(error) => {
            let _ = writeln!(
                log,
                "claude-skills learn: read observations failed: {error}"
            );
            return report;
        }
    };
    if observations.is_empty() {
        return report;
    }

    // 1. Cluster observations into per-project, per-signature aggregates.
    let clusters = cluster_observations(&observations);

    // 2. Upsert one instinct per qualifying cluster.
    let store = RecordStore::new(claude_home, INSTINCT_GROUP);
    let mut trusted_by_project: BTreeMap<String, Vec<TrustedInstinct>> = BTreeMap::new();
    let mut live_ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for cluster in clusters.values() {
        if cluster.count < INSTINCT_MIN_COUNT {
            continue;
        }
        let confidence = (cluster.count as i64).min(INSTINCT_CONFIDENCE_CAP);
        let instinct_id = instinct_id(&cluster.project, &cluster.signature);
        live_ids.insert(instinct_id.clone());
        if !options.dry_run {
            if let Err(error) = write_instinct(&store, &instinct_id, cluster, confidence) {
                let _ = writeln!(log, "claude-skills learn: write instinct failed: {error}");
                continue;
            }
        }
        report.instincts_recorded += 1;

        if confidence >= SKILL_MIN_CONFIDENCE && cluster.distinct_sessions >= 2 {
            trusted_by_project
                .entry(cluster.project.clone())
                .or_default()
                .push(TrustedInstinct {
                    signature: cluster.signature.clone(),
                    guidance: guidance_for(cluster),
                    confidence,
                    count: cluster.count,
                    distinct_sessions: cluster.distinct_sessions,
                });
        }
    }

    // 2b. Decay and prune observed instincts that did not reappear this cycle.
    // A pattern the user stopped doing drops out of the window, so its instinct
    // is no longer refreshed; we decay it and, once it hits the floor, delete it
    // so the store reflects current behavior rather than an ever-growing archive.
    // Manual instincts are never touched. Skipped entirely on a dry run.
    if !options.dry_run {
        report.instincts_pruned = decay_and_prune_instincts(&store, &live_ids, log);
    }

    // 3. Evolve trusted instinct clusters into generated skills + agents.
    for (project, mut instincts) in trusted_by_project {
        if instincts.len() < SKILL_MIN_INSTINCTS {
            continue;
        }
        instincts.sort_by(|a, b| {
            b.confidence
                .cmp(&a.confidence)
                .then(a.signature.cmp(&b.signature))
        });
        match evolve_skill(claude_home, &project, &instincts, options, log) {
            EvolveOutcome::Generated { agent_generated } => {
                report.skills_generated += 1;
                if agent_generated {
                    report.agents_generated += 1;
                }
                report
                    .notes
                    .push(format!("learned-{}: generated", project_slug(&project)));
                if options.synthesize && !options.dry_run {
                    report.synthesis_briefs.push(synthesis_brief(
                        claude_home,
                        &project,
                        &instincts,
                    ));
                }
            }
            EvolveOutcome::Respected => {
                report.skills_respected += 1;
                report.notes.push(format!(
                    "learned-{}: respected manual edit",
                    project_slug(&project)
                ));
            }
            // A skill still at its deterministic-template state (unchanged this
            // cycle) is the prime target for synthesis: the agent has not yet
            // refined it. Emit a brief so a later `learn synthesize` can upgrade
            // its prose even when nothing about the signatures changed.
            EvolveOutcome::Unchanged => {
                if options.synthesize && !options.dry_run {
                    report.synthesis_briefs.push(synthesis_brief(
                        claude_home,
                        &project,
                        &instincts,
                    ));
                }
            }
            EvolveOutcome::Failed(message) => {
                let _ = writeln!(log, "claude-skills learn: {message}");
            }
        }
    }

    report
}

/// Render a compact, always-on digest of the trusted instincts for the project
/// rooted at `cwd`, for injection into SessionStart context. Empty string when
/// there is nothing trusted to surface, so the caller can append unconditionally
/// without adding a blank section.
///
/// This is the lightweight, always-on tier (ECC's key move): generated skills
/// are loaded on demand by Claude Code's matcher, but a one-line-per-instinct
/// digest of what the user reliably does in *this* project is cheap enough to
/// keep in context every session. Only instincts at or above the skill-trust
/// confidence are surfaced, so a half-formed pattern never leaks into context.
pub fn project_instinct_digest(claude_home: &Path, cwd: &str) -> String {
    let project = project_name(cwd);
    let store = RecordStore::new(claude_home, INSTINCT_GROUP);
    let records = match store.list_records() {
        Ok(records) => records,
        Err(_) => return String::new(),
    };
    let mut lines: Vec<(i64, String)> = Vec::new();
    for (_, record) in &records {
        if field(record, "project") != Some(project.as_str()) {
            continue;
        }
        let confidence: i64 = field(record, "confidence")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        if confidence < SKILL_MIN_CONFIDENCE {
            continue;
        }
        let guidance = field(record, "guidance").unwrap_or("").trim();
        if guidance.is_empty() {
            continue;
        }
        lines.push((
            confidence,
            format!("- {guidance} (confidence {confidence})"),
        ));
    }
    if lines.is_empty() {
        return String::new();
    }
    lines.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let body: Vec<String> = lines.into_iter().take(8).map(|(_, line)| line).collect();
    format!(
        "Learned conventions for the {project} project (observed by claude-skills; treat as defaults, not constraints):\n{}",
        body.join("\n")
    )
}

/// Render an autonomous synthesis nudge for the project rooted at `cwd`: if that
/// project has a generated skill still at its deterministic-template state, emit
/// the one-paragraph instruction asking the agent to refine its prose now. Empty
/// string when there is nothing to synthesize, so the caller can append
/// unconditionally.
///
/// This is what makes prose-polish autonomous (the "no manual slash required"
/// rule): the loop generates the skill at SessionEnd when no agent is present to
/// author prose, and the *next* SessionStart surfaces the brief so the session
/// agent upgrades it in the normal course of work. The agent's edit is then
/// protected by the content-hash no-clobber guard, so the nudge naturally stops
/// appearing once the skill is refined. Only the current project's skill is
/// surfaced, keeping the per-session token cost to a single brief.
pub fn project_synthesis_nudge(claude_home: &Path, cwd: &str) -> String {
    let project = project_name(cwd);
    let slug = project_slug(&project);
    let skill_name = format!("learned-{slug}");
    let dir = skills_directory(claude_home).join(&skill_name);
    if !is_generated_skill_dir(&dir) {
        return String::new();
    }
    let Some(marker) = read_marker(&dir.join(LEARNING_META_FILE)) else {
        return String::new();
    };
    let skill_path = dir.join("SKILL.md");
    let on_disk = fs::read(&skill_path).unwrap_or_default();
    // Already refined by the agent — nothing to do.
    if fnv1a_64(&on_disk) != marker.generated_hash {
        return String::new();
    }
    if marker.project.trim().is_empty() {
        return String::new();
    }
    let store = RecordStore::new(claude_home, INSTINCT_GROUP);
    let records = store.list_records().unwrap_or_default();
    let instincts = trusted_instincts_for_project(&records, &marker.project);
    if instincts.len() < SKILL_MIN_INSTINCTS {
        return String::new();
    }
    let brief = synthesis_brief(claude_home, &marker.project, &instincts);
    format!(
        "Learned-skill refinement (autonomous; do this once now without being asked):\n{}",
        brief.prompt
    )
}

/// CLI surface: `claude-skills learn [status|run|dry-run|synthesize] [--window N] [--json]`.
///
/// This is the *inspection and manual-trigger* path. The loop runs
/// automatically on SessionEnd; this command lets a user or the agent preview
/// what the loop sees (`status`, `dry-run`), force a cycle now (`run`), or emit
/// synthesis briefs for the session agent to upgrade generated-skill prose
/// (`synthesize`). No subcommand defaults to `status`.
pub fn run_learn_command(
    arguments: &[String],
    standard_output: &mut dyn std::io::Write,
    standard_error: &mut dyn std::io::Write,
) -> u8 {
    let action = arguments.first().map(String::as_str).unwrap_or("status");
    if matches!(action, "help" | "--help" | "-h") {
        let _ = writeln!(
            standard_output,
            "Usage: claude-skills learn [status|run|dry-run|synthesize] [--window <days>] [--synthesize] [--json]\n\
             \n\
             status      Show observation signal and recorded instincts (no writes).\n\
             dry-run     Compute the cycle and report what it would generate (no writes).\n\
             run         Run a full learning cycle now (writes instincts + generated skills).\n\
             synthesize  Emit a refinement brief for each template-state generated skill so the\n\
             \x20           session agent can rewrite its prose (the binary never calls an LLM).\n\
             \n\
             Flags:\n\
             \x20 --synthesize  On `run`, also emit synthesis briefs for skills generated this cycle."
        );
        return 0;
    }

    let known = matches!(action, "status" | "run" | "dry-run" | "synthesize");
    let rest = if known {
        &arguments[1..]
    } else {
        // No recognized subcommand: treat the whole arg list as flags to status.
        arguments
    };
    let action = if known { action } else { "status" };

    let mut flags = FlagSet::new("learn");
    flags.string_flag("window", OBSERVE_WINDOW_DAYS.to_string());
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    flags.bool_flag("synthesize", false);
    if let Err(error) = flags.parse(rest) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let window_days: u64 = flags
        .string_value("window")
        .trim()
        .parse()
        .unwrap_or(OBSERVE_WINDOW_DAYS);
    let json = flags.bool_value("json");

    let claude_home = match resolve_claude_home(flags.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "learn: {error}");
            return 1;
        }
    };

    match action {
        "status" => learn_status(
            &claude_home,
            window_days,
            json,
            standard_output,
            standard_error,
        ),
        "synthesize" => learn_synthesize(&claude_home, json, standard_output, standard_error),
        "dry-run" | "run" => {
            let options = CycleOptions {
                dry_run: action == "dry-run",
                window_days,
                synthesize: flags.bool_value("synthesize"),
            };
            let report = run_learning_cycle(&claude_home, &options, standard_error);
            if json {
                let briefs: Vec<serde_json::Value> = report
                    .synthesis_briefs
                    .iter()
                    .map(|brief| {
                        serde_json::json!({
                            "skill": brief.skill_name,
                            "path": brief.skill_path,
                            "project": brief.project,
                            "prompt": brief.prompt,
                        })
                    })
                    .collect();
                let payload = serde_json::json!({
                    "action": action,
                    "windowDays": window_days,
                    "instinctsRecorded": report.instincts_recorded,
                    "skillsGenerated": report.skills_generated,
                    "skillsRespected": report.skills_respected,
                    "agentsGenerated": report.agents_generated,
                    "notes": report.notes,
                    "synthesisBriefs": briefs,
                });
                match serde_json::to_string_pretty(&payload) {
                    Ok(text) => {
                        let _ = writeln!(standard_output, "{text}");
                        0
                    }
                    Err(error) => {
                        let _ = writeln!(standard_error, "learn: render json: {error}");
                        1
                    }
                }
            } else {
                let verb = if options.dry_run {
                    "would record"
                } else {
                    "recorded"
                };
                let _ = writeln!(
                    standard_output,
                    "learn {action}: {verb} {} instinct(s); {} skill(s) generated, {} respected, {} agent(s) generated",
                    report.instincts_recorded,
                    report.skills_generated,
                    report.skills_respected,
                    report.agents_generated
                );
                for note in &report.notes {
                    let _ = writeln!(standard_output, "  {note}");
                }
                render_synthesis_briefs(&report.synthesis_briefs, standard_output);
                0
            }
        }
        _ => unreachable!("action normalized above"),
    }
}

/// `learn synthesize`: scan disk for template-state generated skills and emit a
/// refinement brief for each so the session agent can author richer prose. The
/// agent's resulting edit is protected by the content-hash no-clobber guard.
fn learn_synthesize(
    claude_home: &Path,
    json: bool,
    standard_output: &mut dyn std::io::Write,
    standard_error: &mut dyn std::io::Write,
) -> u8 {
    let briefs = collect_synthesis_briefs(claude_home);
    if json {
        let payload = serde_json::json!({
            "action": "synthesize",
            "briefCount": briefs.len(),
            "synthesisBriefs": briefs
                .iter()
                .map(|brief| serde_json::json!({
                    "skill": brief.skill_name,
                    "path": brief.skill_path,
                    "project": brief.project,
                    "prompt": brief.prompt,
                }))
                .collect::<Vec<_>>(),
        });
        match serde_json::to_string_pretty(&payload) {
            Ok(text) => {
                let _ = writeln!(standard_output, "{text}");
                0
            }
            Err(error) => {
                let _ = writeln!(standard_error, "learn: render json: {error}");
                1
            }
        }
    } else {
        if briefs.is_empty() {
            let _ = writeln!(
                standard_output,
                "learn synthesize: no template-state generated skills to refine"
            );
            return 0;
        }
        let _ = writeln!(
            standard_output,
            "learn synthesize: {} skill(s) ready for prose refinement",
            briefs.len()
        );
        render_synthesis_briefs(&briefs, standard_output);
        0
    }
}

/// Render synthesis briefs as agent-actionable text blocks.
fn render_synthesis_briefs(briefs: &[SynthesisBrief], standard_output: &mut dyn std::io::Write) {
    for brief in briefs {
        let _ = writeln!(standard_output, "\n--- synthesize {} ---", brief.skill_name);
        let _ = writeln!(standard_output, "{}", brief.prompt);
    }
}

fn learn_status(
    claude_home: &Path,
    window_days: u64,
    json: bool,
    standard_output: &mut dyn std::io::Write,
    standard_error: &mut dyn std::io::Write,
) -> u8 {
    let observations = observation::iter_recent_rows(window_days).unwrap_or_default();
    let clusters = cluster_observations(&observations);
    let mut qualifying: Vec<&Cluster> = clusters
        .values()
        .filter(|cluster| cluster.count >= INSTINCT_MIN_COUNT)
        .collect();
    qualifying.sort_by(|a, b| b.count.cmp(&a.count).then(a.signature.cmp(&b.signature)));

    let store = RecordStore::new(claude_home, INSTINCT_GROUP);
    let instincts = store.list_records().unwrap_or_default();
    let observed_instincts = instincts
        .iter()
        .filter(|(_, record)| field(record, "source") == Some(SOURCE_OBSERVED))
        .count();

    if json {
        let signals: Vec<serde_json::Value> = qualifying
            .iter()
            .map(|cluster| {
                serde_json::json!({
                    "project": cluster.project,
                    "signature": cluster.signature,
                    "count": cluster.count,
                    "sessions": cluster.distinct_sessions,
                })
            })
            .collect();
        let payload = serde_json::json!({
            "windowDays": window_days,
            "observations": observations.len(),
            "qualifyingSignals": signals,
            "recordedInstincts": instincts.len(),
            "observedInstincts": observed_instincts,
        });
        match serde_json::to_string_pretty(&payload) {
            Ok(text) => {
                let _ = writeln!(standard_output, "{text}");
                0
            }
            Err(error) => {
                let _ = writeln!(standard_error, "learn: render json: {error}");
                1
            }
        }
    } else {
        let _ = writeln!(
            standard_output,
            "learn status (last {window_days}d): {} observation(s), {} qualifying signal(s), {} instinct(s) recorded ({} observed)",
            observations.len(),
            qualifying.len(),
            instincts.len(),
            observed_instincts
        );
        for cluster in qualifying.iter().take(20) {
            let _ = writeln!(
                standard_output,
                "  [{}× / {} sess] {} :: {}",
                cluster.count, cluster.distinct_sessions, cluster.project, cluster.signature
            );
        }
        0
    }
}

/// A windowed aggregate for one (project, signature) pair.
struct Cluster {
    project: String,
    signature: String,
    count: u64,
    distinct_sessions: usize,
    /// Most recent human-readable detail, for the skill body.
    sample_detail: String,
    /// Dominant tool name (Bash / Edit / ...), for guidance phrasing.
    tool_name: String,
}

/// A trusted instinct contributing to a generated skill.
struct TrustedInstinct {
    signature: String,
    guidance: String,
    confidence: i64,
    count: u64,
    distinct_sessions: usize,
}

fn cluster_observations(observations: &[Observation]) -> BTreeMap<String, Cluster> {
    let mut clusters: BTreeMap<String, Cluster> = BTreeMap::new();
    let mut sessions: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
    for observation in observations {
        let project = project_name(&observation.cwd);
        let key = format!("{project}\u{1f}{}", observation.signature);
        let cluster = clusters.entry(key.clone()).or_insert_with(|| Cluster {
            project: project.clone(),
            signature: observation.signature.clone(),
            count: 0,
            distinct_sessions: 0,
            sample_detail: String::new(),
            tool_name: observation.tool_name.clone(),
        });
        cluster.count += 1;
        if !observation.detail.is_empty() {
            cluster.sample_detail = observation.detail.clone();
        }
        sessions
            .entry(key)
            .or_default()
            .insert(observation.session_id.clone());
    }
    for (key, cluster) in clusters.iter_mut() {
        cluster.distinct_sessions = sessions.get(key).map(|set| set.len()).unwrap_or(0);
    }
    clusters
}

/// Derive a project name from an absolute cwd. The last path component is a
/// stable, human-meaningful label on a single-user machine. Empty cwd (older
/// hook inputs that omit it) maps to a shared `global` bucket.
fn project_name(cwd: &str) -> String {
    let trimmed = cwd.trim().trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return "global".to_string();
    }
    trimmed
        .rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or("global")
        .to_string()
}

fn project_slug(project: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for character in project.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
    }
    let trimmed = slug.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "project".to_string()
    } else {
        trimmed
    }
}

fn instinct_id(project: &str, signature: &str) -> String {
    project_slug(&format!("{project} {signature}"))
}

fn guidance_for(cluster: &Cluster) -> String {
    match cluster.tool_name.as_str() {
        "Bash" => format!("Frequently runs `{}` in this project", cluster.signature),
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            let extension = cluster.signature.strip_prefix("edit:").unwrap_or("files");
            format!("Most edits in this project target `.{extension}` files")
        }
        _ => format!("Repeated `{}` in this project", cluster.signature),
    }
}

fn write_instinct(
    store: &RecordStore,
    id: &str,
    cluster: &Cluster,
    confidence: i64,
) -> Result<(), String> {
    // Respect provenance: never rewrite a manually-authored instinct that
    // happens to share this id. The loop only owns records it marked observed.
    if let Some(existing) = store.read_record(id)? {
        let source = field(&existing, "source").unwrap_or("");
        if source != SOURCE_OBSERVED {
            return Ok(());
        }
    }
    let record: Record = vec![
        ("id".into(), id.to_string()),
        ("trigger".into(), cluster.signature.clone()),
        ("guidance".into(), guidance_for(cluster)),
        ("confidence".into(), confidence.to_string()),
        ("observations".into(), cluster.count.to_string()),
        ("sessions".into(), cluster.distinct_sessions.to_string()),
        ("project".into(), cluster.project.clone()),
        ("source".into(), SOURCE_OBSERVED.to_string()),
        ("sample".into(), cluster.sample_detail.clone()),
    ];
    store.write_record(id, &record)?;
    Ok(())
}

/// Decay every observed instinct that was NOT refreshed this cycle, and delete
/// the ones that reach the prune floor. `live_ids` are the instincts the current
/// window still supports. Returns the count deleted.
///
/// This is the natural counterpart to `write_instinct`: confidence rises while a
/// habit recurs and falls once it stops, so the store self-trims to current
/// behavior. Manual instincts (`source != observed`) are skipped entirely — the
/// loop never deletes what a human authored.
fn decay_and_prune_instincts(
    store: &RecordStore,
    live_ids: &std::collections::BTreeSet<String>,
    log: &mut dyn std::io::Write,
) -> usize {
    let records = match store.list_records() {
        Ok(records) => records,
        Err(error) => {
            let _ = writeln!(log, "claude-skills learn: list instincts failed: {error}");
            return 0;
        }
    };
    let mut pruned = 0usize;
    for (id, mut record) in records {
        if field(&record, "source") != Some(SOURCE_OBSERVED) {
            continue; // never decay or delete a manual instinct
        }
        if live_ids.contains(&id) {
            continue; // refreshed this cycle by write_instinct
        }
        let confidence: i64 = field(&record, "confidence")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let decayed = confidence - 1;
        if decayed <= INSTINCT_PRUNE_FLOOR {
            match store.delete_record(&id) {
                Ok(_) => pruned += 1,
                Err(error) => {
                    let _ = writeln!(log, "claude-skills learn: prune instinct failed: {error}");
                }
            }
        } else {
            if let Some(slot) = record.iter_mut().find(|(key, _)| key == "confidence") {
                slot.1 = decayed.to_string();
            } else {
                record.push(("confidence".to_string(), decayed.to_string()));
            }
            if let Err(error) = store.write_record(&id, &record) {
                let _ = writeln!(log, "claude-skills learn: decay instinct failed: {error}");
            }
        }
    }
    pruned
}

enum EvolveOutcome {
    Generated { agent_generated: bool },
    Respected,
    Unchanged,
    Failed(String),
}

fn evolve_skill(
    claude_home: &Path,
    project: &str,
    instincts: &[TrustedInstinct],
    options: &CycleOptions,
    log: &mut dyn std::io::Write,
) -> EvolveOutcome {
    let slug = project_slug(project);
    let skill_name = format!("learned-{slug}");
    let skill_dir = skills_directory(claude_home).join(&skill_name);
    let skill_path = skill_dir.join("SKILL.md");
    let meta_path = skill_dir.join(LEARNING_META_FILE);

    let signature_set = signature_set(instincts);
    let content = render_skill(&skill_name, project, instincts);
    let content_hash = fnv1a_64(content.as_bytes());

    // Decide whether to (re)write.
    if skill_dir.exists() {
        let marker = read_marker(&meta_path);
        match marker {
            Some(marker) => {
                // No-clobber guard: if the on-disk SKILL.md differs from what we
                // last generated, a human or the agent refined it — leave it be.
                let on_disk = fs::read(&skill_path).unwrap_or_default();
                if fnv1a_64(&on_disk) != marker.generated_hash {
                    return EvolveOutcome::Respected;
                }
                // Untouched since we wrote it. Only regenerate if the underlying
                // signature set actually changed; otherwise it is identical work.
                if marker.signature_set == signature_set {
                    return EvolveOutcome::Unchanged;
                }
            }
            None => {
                // A directory named `learned-*` with no marker is not ours
                // (e.g. a user authored a skill with that name). Never touch it.
                return EvolveOutcome::Respected;
            }
        }
    }

    if options.dry_run {
        let _ = writeln!(log, "claude-skills learn: would generate {skill_name}");
        return EvolveOutcome::Generated {
            agent_generated: false,
        };
    }

    if let Err(error) = fs::create_dir_all(&skill_dir) {
        return EvolveOutcome::Failed(format!("create {}: {error}", display_path(&skill_dir)));
    }
    if let Err(error) = fs::write(&skill_path, content.as_bytes()) {
        return EvolveOutcome::Failed(format!("write {}: {error}", display_path(&skill_path)));
    }
    if let Err(error) = write_marker(&meta_path, content_hash, &signature_set, project) {
        return EvolveOutcome::Failed(error);
    }

    // Scaffold a subagent that preloads the new skill, marked generated.
    let agent_generated = match generate_agent(claude_home, &skill_name, project, log) {
        Ok(generated) => generated,
        Err(error) => {
            let _ = writeln!(log, "claude-skills learn: {error}");
            false
        }
    };

    EvolveOutcome::Generated { agent_generated }
}

/// Build the synthesis brief for one generated skill: a concrete, ready-to-use
/// instruction the session agent fulfils to replace the deterministic template
/// prose with richer, project-specific prose. The binary never calls an LLM —
/// this is the bridge that lets the *session* model (which Claude Code already
/// runs) do the authoring, while provenance keeps the result safe from the next
/// cycle. The brief embeds the observed conventions so the agent has the full
/// signal without re-reading the instinct store.
fn synthesis_brief(
    claude_home: &Path,
    project: &str,
    instincts: &[TrustedInstinct],
) -> SynthesisBrief {
    let slug = project_slug(project);
    let skill_name = format!("learned-{slug}");
    let skill_path = skills_directory(claude_home)
        .join(&skill_name)
        .join("SKILL.md");
    let mut observed = String::new();
    for instinct in instincts {
        observed.push_str(&format!(
            "  - {} (observed {}× across {} session(s), confidence {})\n",
            instinct.guidance, instinct.count, instinct.distinct_sessions, instinct.confidence
        ));
    }
    let prompt = format!(
        "Refine the generated skill `{skill_name}` at {path}.\n\
         It currently holds a deterministic template. Rewrite its prose so it reads like a \
         hand-authored skill for the `{project}` project, using these observed conventions as the \
         source of truth:\n{observed}\
         Requirements: keep the YAML frontmatter intact (name, description, when_to_use, \
         generated: true, provenance: learned must all remain); keep the body factual and specific \
         to these conventions; do not invent commands or files that are not implied by the \
         observations above; prefer concrete \"when X, do Y\" guidance over generic advice. \
         Your edit is protected — the learning loop detects the content change by hash and will \
         not overwrite it.",
        path = display_path(&skill_path),
    );
    SynthesisBrief {
        skill_name,
        skill_path: display_path(&skill_path),
        project: project.to_string(),
        prompt,
    }
}

fn signature_set(instincts: &[TrustedInstinct]) -> String {
    let mut signatures: Vec<&str> = instincts.iter().map(|i| i.signature.as_str()).collect();
    signatures.sort_unstable();
    signatures.join("\n")
}

/// Render the deterministic SKILL.md body for a project's trusted instincts.
fn render_skill(skill_name: &str, project: &str, instincts: &[TrustedInstinct]) -> String {
    let mut body = String::new();
    body.push_str("---\n");
    body.push_str(&format!("name: {skill_name}\n"));
    body.push_str(&format!(
        "description: Learned workflow conventions for the {project} project — repeated command and edit patterns observed by claude-skills. Auto-generated from your behavior; safe for the agent to refine.\n"
    ));
    body.push_str(&format!(
        "when_to_use: When working in the {project} project, to follow its established command and file conventions without re-deriving them.\n"
    ));
    body.push_str("generated: true\n");
    body.push_str("generator: claude-skills-learning\n");
    body.push_str("provenance: learned\n");
    body.push_str("---\n\n");
    body.push_str(&format!("# Learned workflow: {project}\n\n"));
    body.push_str(
        "This skill was generated automatically by observing repeated actions in this project. \
It is safe to refine or replace: the claude-skills learning loop detects manual edits by content \
hash and will not overwrite them. Built-in skills are never modified by the loop.\n\n",
    );
    body.push_str("## Observed conventions\n\n");
    for instinct in instincts {
        body.push_str(&format!(
            "- {} (observed {}× across {} session(s), confidence {})\n",
            instinct.guidance, instinct.count, instinct.distinct_sessions, instinct.confidence
        ));
    }
    body.push_str(
        "\n## How to use\n\nWhen acting in this project, prefer the conventions above unless the \
current task calls for something different. Treat them as defaults, not constraints.\n",
    );
    body
}

/// Generate a subagent `.md` that preloads the generated skill. Returns whether
/// a new agent file was written (false when one already exists and is ours).
fn generate_agent(
    claude_home: &Path,
    skill_name: &str,
    project: &str,
    log: &mut dyn std::io::Write,
) -> Result<bool, String> {
    let agent_path = agents_directory(claude_home).join(format!("{skill_name}.md"));
    if agent_path.exists() {
        // Only manage agents we generated; a same-named manual agent is left be.
        let existing = fs::read_to_string(&agent_path).unwrap_or_default();
        if !existing.contains("generated: true") {
            return Ok(false);
        }
    }
    if let Some(parent) = agent_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", display_path(parent)))?;
    }
    let mut body = String::new();
    body.push_str("---\n");
    body.push_str(&format!("name: {skill_name}\n"));
    body.push_str(&format!(
        "description: Works within the {project} project using its learned conventions. Auto-generated by claude-skills.\n"
    ));
    body.push_str("generated: true\n");
    body.push_str("skills:\n");
    body.push_str(&format!("  - {skill_name}\n"));
    body.push_str("---\n\n");
    body.push_str(&format!(
        "You assist with work in the {project} project. The `{skill_name}` skill is preloaded \
with this project's observed command and file conventions; follow it as the default working style.\n"
    ));
    fs::write(&agent_path, body.as_bytes())
        .map_err(|error| format!("write {}: {error}", display_path(&agent_path)))?;
    let _ = writeln!(log, "claude-skills learn: generated agent {skill_name}");
    Ok(true)
}

/// Sidecar marker recording what the loop last generated, for the no-clobber guard.
struct LearningMarker {
    generated_hash: u64,
    signature_set: String,
    project: String,
}

fn read_marker(path: &Path) -> Option<LearningMarker> {
    let text = fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let generated_hash = value.get("generatedHash").and_then(|v| v.as_str())?;
    let signature_set = value
        .get("signatureSet")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let project = value
        .get("project")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    Some(LearningMarker {
        generated_hash: generated_hash.parse().ok()?,
        signature_set,
        project,
    })
}

fn write_marker(
    path: &Path,
    content_hash: u64,
    signature_set: &str,
    project: &str,
) -> Result<(), String> {
    let value = serde_json::json!({
        "generator": "claude-skills-learning",
        "generatedHash": content_hash.to_string(),
        "signatureSet": signature_set,
        "project": project,
    });
    let serialized = serde_json::to_string_pretty(&value)
        .map_err(|error| format!("serialize marker: {error}"))?;
    fs::write(path, serialized).map_err(|error| format!("write {}: {error}", display_path(path)))
}

/// Whether a skills directory entry is a loop-generated skill. Used by uninstall
/// and curation to act only on generated artifacts, never built-in ones.
pub fn is_generated_skill_dir(skill_dir: &Path) -> bool {
    skill_dir.join(LEARNING_META_FILE).is_file()
}

/// Scan disk for generated skills still at their deterministic-template state and
/// build a synthesis brief for each. Unlike collecting briefs mid-cycle, this
/// path works on skills generated in *prior* sessions (the common case — the loop
/// runs at SessionEnd when no agent is present to author prose, so synthesis is a
/// separate, agent-present step).
///
/// "Template state" = the on-disk SKILL.md hash still equals the marker's
/// `generatedHash`. A skill the agent already refined (hash differs) is skipped:
/// it is no longer a template, and re-synthesizing would fight the agent's work.
/// The trusted instincts behind each skill are reconstructed from the instinct
/// store so the brief carries the same observed-convention signal a fresh
/// generation would.
pub fn collect_synthesis_briefs(claude_home: &Path) -> Vec<SynthesisBrief> {
    let mut briefs = Vec::new();
    let skills_root = skills_directory(claude_home);
    let Ok(entries) = fs::read_dir(&skills_root) else {
        return briefs;
    };
    let store = RecordStore::new(claude_home, INSTINCT_GROUP);
    let records = store.list_records().unwrap_or_default();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let meta_path = dir.join(LEARNING_META_FILE);
        let Some(marker) = read_marker(&meta_path) else {
            continue; // not a loop-generated skill
        };
        // Only template-state skills (untouched since we wrote them) are eligible.
        let skill_path = dir.join("SKILL.md");
        let on_disk = fs::read(&skill_path).unwrap_or_default();
        if fnv1a_64(&on_disk) != marker.generated_hash {
            continue; // already refined — leave the agent's prose alone
        }
        if marker.project.trim().is_empty() {
            continue; // pre-project-tag marker; nothing reliable to synthesize from
        }
        let instincts = trusted_instincts_for_project(&records, &marker.project);
        if instincts.len() < SKILL_MIN_INSTINCTS {
            continue;
        }
        briefs.push(synthesis_brief(claude_home, &marker.project, &instincts));
    }
    briefs.sort_by(|a, b| a.skill_name.cmp(&b.skill_name));
    briefs
}

/// Reconstruct the trusted-instinct set for a project from the instinct store,
/// matching the in-cycle trust bar (confidence >= SKILL_MIN_CONFIDENCE, seen
/// across >= 2 sessions). Used by the disk-scan synthesis path.
fn trusted_instincts_for_project(
    records: &[(String, Record)],
    project: &str,
) -> Vec<TrustedInstinct> {
    let mut instincts = Vec::new();
    for (_, record) in records {
        if field(record, "project") != Some(project) {
            continue;
        }
        let confidence: i64 = field(record, "confidence")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        if confidence < SKILL_MIN_CONFIDENCE {
            continue;
        }
        let distinct_sessions: usize = field(record, "sessions")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        if distinct_sessions < 2 {
            continue;
        }
        let count: u64 = field(record, "observations")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        instincts.push(TrustedInstinct {
            signature: field(record, "trigger").unwrap_or("").to_string(),
            guidance: field(record, "guidance").unwrap_or("").to_string(),
            confidence,
            count,
            distinct_sessions,
        });
    }
    instincts.sort_by(|a, b| {
        b.confidence
            .cmp(&a.confidence)
            .then(a.signature.cmp(&b.signature))
    });
    instincts
}

/// Remove every loop-generated skill and its paired subagent under `claude_home`.
/// Returns the count removed. Built-in (repo-synced) skills are identified by the
/// absence of the marker file and are never touched.
pub fn remove_generated_artifacts(claude_home: &Path) -> std::io::Result<usize> {
    let mut removed = 0usize;
    let skills_root = skills_directory(claude_home);
    if skills_root.is_dir() {
        for entry in fs::read_dir(&skills_root)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() || !is_generated_skill_dir(&path) {
                continue;
            }
            let skill_name = entry.file_name();
            fs::remove_dir_all(&path)?;
            removed += 1;
            // Remove the paired generated agent if present and ours.
            if let Some(name) = skill_name.to_str() {
                let agent_path = agents_directory(claude_home).join(format!("{name}.md"));
                if agent_path.is_file() {
                    let body = fs::read_to_string(&agent_path).unwrap_or_default();
                    if body.contains("generated: true") {
                        fs::remove_file(&agent_path)?;
                    }
                }
            }
        }
    }
    Ok(removed)
}

/// FNV-1a 64-bit. Dependency-free and stable across runs/platforms, which the
/// no-clobber guard needs: the same content must always hash identically so a
/// later cycle can tell "untouched" from "manually refined".
fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::observation;
    use crate::test_support::ENV_LOCK;
    use serde_json::json;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Assert a path is gone, tolerating Windows' delete-pending state.
    ///
    /// `fs::remove_dir_all` returns `Ok` once the delete is *posted*, but on
    /// Windows the directory entry can linger briefly while another handle
    /// (an antivirus scan, the indexer, or a sibling test thread) closes. An
    /// instantaneous `!exists()` check therefore races under parallel `cargo
    /// test` load — the source of an intermittent failure in
    /// `remove_generated_artifacts_leaves_builtin_skills`. Poll for a short,
    /// bounded window so the assertion verifies the real contract ("the path
    /// is removed") without depending on the OS reclaiming the entry within a
    /// single instruction. On Unix the first check passes immediately.
    fn assert_removed_eventually(path: &Path) {
        for _ in 0..50 {
            if !path.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!(
            "path was not removed within the delete-pending window: {}",
            path.display()
        );
    }

    fn isolated_home<F: FnOnce(&PathBuf)>(suffix: &str, run: F) {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "claude-skills-learning-{}-{nanos}-{suffix}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create test claude home");
        let previous = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &root);
        run(&root);
        match previous {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// Seed `count` Bash observations of `command` across `sessions` distinct
    /// sessions, cwd fixed to `/work/<project>`.
    fn seed_bash(project: &str, command: &str, count: usize, sessions: usize) {
        for index in 0..count {
            let session = format!("s{}", index % sessions.max(1));
            let input = json!({
                "tool_name": "Bash",
                "session_id": session,
                "cwd": format!("/work/{project}"),
                "tool_input": { "command": command },
            });
            observation::record_observation(&input).expect("record");
        }
    }

    #[test]
    fn project_name_takes_last_path_segment() {
        assert_eq!(project_name("/work/myproj"), "myproj");
        assert_eq!(project_name("C:\\Users\\x\\repo\\"), "repo");
        assert_eq!(project_name(""), "global");
    }

    #[test]
    fn cycle_records_instinct_above_min_count() {
        isolated_home("instinct", |root| {
            seed_bash("alpha", "cargo test --workspace", 4, 2);
            let mut log = Vec::new();
            let report = run_learning_cycle(root, &CycleOptions::default(), &mut log);
            assert_eq!(report.instincts_recorded, 1);
            let store = RecordStore::new(root, INSTINCT_GROUP);
            let id = instinct_id("alpha", "cargo test");
            let record = store.read_record(&id).expect("read").expect("exists");
            assert_eq!(field(&record, "source"), Some("observed"));
            assert_eq!(field(&record, "trigger"), Some("cargo test"));
        });
    }

    #[test]
    fn cycle_skips_signature_below_min_count() {
        isolated_home("below-min", |root| {
            seed_bash("beta", "git push", 2, 1);
            let mut log = Vec::new();
            let report = run_learning_cycle(root, &CycleOptions::default(), &mut log);
            assert_eq!(report.instincts_recorded, 0);
        });
    }

    #[test]
    fn cycle_decays_then_prunes_stale_observed_instinct() {
        isolated_home("decay", |root| {
            let store = RecordStore::new(root, INSTINCT_GROUP);
            // A stale observed instinct at confidence 1 whose signature is NOT in
            // the current window. It should decay to 0 and be pruned this cycle.
            let stale_id = instinct_id("oldproj", "git push");
            let stale: Record = vec![
                ("id".into(), stale_id.clone()),
                ("trigger".into(), "git push".into()),
                ("guidance".into(), "x".into()),
                ("confidence".into(), "1".into()),
                ("source".into(), SOURCE_OBSERVED.into()),
            ];
            store.write_record(&stale_id, &stale).expect("seed stale");
            // A higher-confidence stale instinct decays but survives one cycle.
            let surviving_id = instinct_id("oldproj", "cargo build");
            let surviving: Record = vec![
                ("id".into(), surviving_id.clone()),
                ("trigger".into(), "cargo build".into()),
                ("guidance".into(), "x".into()),
                ("confidence".into(), "5".into()),
                ("source".into(), SOURCE_OBSERVED.into()),
            ];
            store
                .write_record(&surviving_id, &surviving)
                .expect("seed surviving");
            // Some live signal so the cycle does not early-return on empty.
            seed_bash("newproj", "cargo test", 4, 2);

            let mut log = Vec::new();
            let report = run_learning_cycle(root, &CycleOptions::default(), &mut log);
            assert_eq!(
                report.instincts_pruned, 1,
                "the confidence-1 instinct is pruned"
            );
            assert!(
                store.read_record(&stale_id).expect("read").is_none(),
                "confidence-1 stale instinct must be deleted"
            );
            let survived = store
                .read_record(&surviving_id)
                .expect("read")
                .expect("survives");
            assert_eq!(
                field(&survived, "confidence"),
                Some("4"),
                "confidence-5 instinct decays to 4, not deleted"
            );
        });
    }

    #[test]
    fn project_instinct_digest_surfaces_only_trusted_for_matching_project() {
        isolated_home("digest", |root| {
            let store = RecordStore::new(root, INSTINCT_GROUP);
            // Trusted instinct for the target project.
            let trusted_id = instinct_id("digestproj", "cargo test");
            store
                .write_record(
                    &trusted_id,
                    &vec![
                        ("id".into(), trusted_id.clone()),
                        ("trigger".into(), "cargo test".into()),
                        ("guidance".into(), "Frequently runs `cargo test`".into()),
                        ("confidence".into(), "6".into()),
                        ("project".into(), "digestproj".into()),
                        ("source".into(), SOURCE_OBSERVED.into()),
                    ],
                )
                .expect("write trusted");
            // Below-threshold instinct (same project) — must NOT surface.
            let weak_id = instinct_id("digestproj", "ls");
            store
                .write_record(
                    &weak_id,
                    &vec![
                        ("id".into(), weak_id.clone()),
                        ("trigger".into(), "ls".into()),
                        ("guidance".into(), "Frequently runs `ls`".into()),
                        ("confidence".into(), "2".into()),
                        ("project".into(), "digestproj".into()),
                        ("source".into(), SOURCE_OBSERVED.into()),
                    ],
                )
                .expect("write weak");
            // Trusted instinct for a DIFFERENT project — must NOT surface.
            let other_id = instinct_id("otherproj", "npm test");
            store
                .write_record(
                    &other_id,
                    &vec![
                        ("id".into(), other_id.clone()),
                        ("trigger".into(), "npm test".into()),
                        ("guidance".into(), "Frequently runs `npm test`".into()),
                        ("confidence".into(), "9".into()),
                        ("project".into(), "otherproj".into()),
                        ("source".into(), SOURCE_OBSERVED.into()),
                    ],
                )
                .expect("write other");

            let digest = project_instinct_digest(root, "/work/digestproj");
            assert!(digest.contains("digestproj"), "names the project: {digest}");
            assert!(digest.contains("cargo test"), "includes trusted instinct");
            assert!(
                !digest.contains("`ls`"),
                "excludes below-threshold instinct"
            );
            assert!(!digest.contains("npm test"), "excludes other project");

            // A project with no trusted instincts yields empty (no blank section).
            assert!(project_instinct_digest(root, "/work/emptyproj").is_empty());
        });
    }

    #[test]
    fn cycle_generates_skill_and_agent_when_cluster_trusted() {
        isolated_home("skill", |root| {
            // Two trusted signatures, each 6× across 2 sessions -> confidence 6.
            seed_bash("gamma", "cargo test --workspace", 6, 2);
            seed_bash("gamma", "git commit -m x", 6, 2);
            let mut log = Vec::new();
            let report = run_learning_cycle(root, &CycleOptions::default(), &mut log);
            assert_eq!(report.skills_generated, 1, "notes: {:?}", report.notes);
            assert_eq!(report.agents_generated, 1);

            let skill_path = skills_directory(root)
                .join("learned-gamma")
                .join("SKILL.md");
            let body = fs::read_to_string(&skill_path).expect("skill written");
            assert!(body.contains("name: learned-gamma"));
            assert!(body.contains("generated: true"));
            assert!(body.contains("cargo test"));
            assert!(body.contains("git commit"));
            assert!(is_generated_skill_dir(
                &skills_directory(root).join("learned-gamma")
            ));

            let agent_path = agents_directory(root).join("learned-gamma.md");
            let agent = fs::read_to_string(&agent_path).expect("agent written");
            assert!(agent.contains("skills:"));
            assert!(agent.contains("- learned-gamma"));
        });
    }

    #[test]
    fn cycle_needs_two_sessions_to_trust_for_skill() {
        isolated_home("one-session", |root| {
            // 8 observations but all one session -> instinct yes, skill no.
            seed_bash("delta", "cargo test", 8, 1);
            seed_bash("delta", "git commit", 8, 1);
            let mut log = Vec::new();
            let report = run_learning_cycle(root, &CycleOptions::default(), &mut log);
            assert_eq!(report.skills_generated, 0);
            assert!(report.instincts_recorded >= 2);
        });
    }

    #[test]
    fn second_cycle_is_idempotent_when_signatures_unchanged() {
        isolated_home("idempotent", |root| {
            seed_bash("eps", "cargo test", 6, 2);
            seed_bash("eps", "git commit", 6, 2);
            let mut log = Vec::new();
            let first = run_learning_cycle(root, &CycleOptions::default(), &mut log);
            assert_eq!(first.skills_generated, 1);
            let second = run_learning_cycle(root, &CycleOptions::default(), &mut log);
            assert_eq!(second.skills_generated, 0, "no regen when unchanged");
            assert_eq!(second.skills_respected, 0);
        });
    }

    #[test]
    fn cycle_respects_manual_edit_to_generated_skill() {
        isolated_home("respect", |root| {
            seed_bash("zeta", "cargo test", 6, 2);
            seed_bash("zeta", "git commit", 6, 2);
            let mut log = Vec::new();
            run_learning_cycle(root, &CycleOptions::default(), &mut log);
            // Simulate the agent refining the generated skill.
            let skill_path = skills_directory(root).join("learned-zeta").join("SKILL.md");
            fs::write(&skill_path, "---\nname: learned-zeta\n---\nhand edited\n").expect("edit");
            // Add a new signature so the loop would otherwise want to regenerate.
            seed_bash("zeta", "cargo build", 6, 2);
            let report = run_learning_cycle(root, &CycleOptions::default(), &mut log);
            assert_eq!(report.skills_generated, 0);
            assert_eq!(report.skills_respected, 1);
            let body = fs::read_to_string(&skill_path).expect("read");
            assert!(body.contains("hand edited"), "manual edit must survive");
        });
    }

    #[test]
    fn never_clobbers_manual_instinct_sharing_id() {
        isolated_home("manual-instinct", |root| {
            let store = RecordStore::new(root, INSTINCT_GROUP);
            let id = instinct_id("eta", "cargo test");
            let manual: Record = vec![
                ("id".into(), id.clone()),
                ("trigger".into(), "cargo test".into()),
                ("guidance".into(), "MANUAL GUIDANCE".into()),
                ("confidence".into(), "99".into()),
            ];
            store.write_record(&id, &manual).expect("seed manual");
            seed_bash("eta", "cargo test", 6, 2);
            let mut log = Vec::new();
            run_learning_cycle(root, &CycleOptions::default(), &mut log);
            let after = store.read_record(&id).expect("read").expect("exists");
            assert_eq!(field(&after, "guidance"), Some("MANUAL GUIDANCE"));
            assert_eq!(field(&after, "confidence"), Some("99"));
        });
    }

    #[test]
    fn remove_generated_artifacts_leaves_builtin_skills() {
        isolated_home("remove", |root| {
            // Built-in skill: a skill dir with no marker.
            let builtin = skills_directory(root).join("reviewer");
            fs::create_dir_all(&builtin).expect("mkdir builtin");
            fs::write(builtin.join("SKILL.md"), "---\nname: reviewer\n---\n").expect("write");
            // Generated skill.
            seed_bash("theta", "cargo test", 6, 2);
            seed_bash("theta", "git commit", 6, 2);
            let mut log = Vec::new();
            run_learning_cycle(root, &CycleOptions::default(), &mut log);
            assert!(skills_directory(root).join("learned-theta").exists());

            let removed = remove_generated_artifacts(root).expect("remove");
            assert_eq!(removed, 1);
            assert_removed_eventually(&skills_directory(root).join("learned-theta"));
            assert!(builtin.exists(), "built-in skill must survive");
            assert_removed_eventually(&agents_directory(root).join("learned-theta.md"));
        });
    }

    #[test]
    fn dry_run_writes_nothing() {
        isolated_home("dry", |root| {
            seed_bash("iota", "cargo test", 6, 2);
            seed_bash("iota", "git commit", 6, 2);
            let mut log = Vec::new();
            let options = CycleOptions {
                dry_run: true,
                window_days: OBSERVE_WINDOW_DAYS,
                synthesize: false,
            };
            let report = run_learning_cycle(root, &options, &mut log);
            assert!(report.instincts_recorded >= 2);
            assert_eq!(report.skills_generated, 1, "dry-run reports intent");
            assert!(
                !skills_directory(root).join("learned-iota").exists(),
                "dry-run must not write the skill"
            );
            let store = RecordStore::new(root, INSTINCT_GROUP);
            assert!(
                store
                    .read_record(&instinct_id("iota", "cargo test"))
                    .expect("read")
                    .is_none(),
                "dry-run must not write instincts"
            );
        });
    }

    #[test]
    fn fnv1a_is_stable_and_distinguishes() {
        assert_eq!(fnv1a_64(b"abc"), fnv1a_64(b"abc"));
        assert_ne!(fnv1a_64(b"abc"), fnv1a_64(b"abd"));
    }

    #[test]
    fn synthesize_emits_brief_for_template_state_generated_skill() {
        isolated_home("synth", |root| {
            seed_bash("synthproj", "cargo test", 6, 2);
            seed_bash("synthproj", "git commit", 6, 2);
            let mut log = Vec::new();
            // First cycle generates the skill at its deterministic-template state.
            let report = run_learning_cycle(root, &CycleOptions::default(), &mut log);
            assert_eq!(report.skills_generated, 1);

            // Disk-scan synthesis sees one template-state skill and builds a brief.
            let briefs = collect_synthesis_briefs(root);
            assert_eq!(briefs.len(), 1, "one template-state skill -> one brief");
            let brief = &briefs[0];
            assert_eq!(brief.skill_name, "learned-synthproj");
            assert!(brief.prompt.contains("learned-synthproj"));
            assert!(
                brief.prompt.contains("cargo test"),
                "brief carries observed conventions: {}",
                brief.prompt
            );
            assert!(
                brief.prompt.contains("frontmatter"),
                "brief states the guardrails"
            );
        });
    }

    #[test]
    fn synthesize_skips_skill_already_refined_by_agent() {
        isolated_home("synth-refined", |root| {
            seed_bash("refinedproj", "cargo test", 6, 2);
            seed_bash("refinedproj", "git commit", 6, 2);
            let mut log = Vec::new();
            run_learning_cycle(root, &CycleOptions::default(), &mut log);

            // Simulate the agent refining the prose (hash now differs from marker).
            let skill_path = skills_directory(root)
                .join("learned-refinedproj")
                .join("SKILL.md");
            fs::write(
                &skill_path,
                "---\nname: learned-refinedproj\ngenerated: true\nprovenance: learned\n---\nHand-authored prose.\n",
            )
            .expect("refine");

            let briefs = collect_synthesis_briefs(root);
            assert!(
                briefs.is_empty(),
                "a skill the agent already refined must not be re-synthesized"
            );
        });
    }

    #[test]
    fn synthesize_skips_builtin_skill() {
        isolated_home("synth-builtin", |root| {
            // A built-in skill (no marker file) must never produce a synthesis brief.
            let builtin = skills_directory(root).join("reviewer");
            fs::create_dir_all(&builtin).expect("mkdir builtin");
            fs::write(builtin.join("SKILL.md"), "---\nname: reviewer\n---\n").expect("write");
            let briefs = collect_synthesis_briefs(root);
            assert!(briefs.is_empty(), "built-in skills are never synthesized");
        });
    }

    #[test]
    fn run_with_synthesize_option_collects_briefs_inline() {
        isolated_home("synth-inline", |root| {
            seed_bash("inlineproj", "cargo test", 6, 2);
            seed_bash("inlineproj", "git commit", 6, 2);
            let mut log = Vec::new();
            let options = CycleOptions {
                dry_run: false,
                window_days: OBSERVE_WINDOW_DAYS,
                synthesize: true,
            };
            let report = run_learning_cycle(root, &options, &mut log);
            assert_eq!(report.skills_generated, 1);
            assert_eq!(
                report.synthesis_briefs.len(),
                1,
                "synthesize option emits a brief for the freshly generated skill"
            );
            assert_eq!(report.synthesis_briefs[0].project, "inlineproj");
        });
    }

    #[test]
    fn synthesis_nudge_surfaces_for_template_skill_then_self_clears() {
        isolated_home("synth-nudge", |root| {
            seed_bash("nudgeproj", "cargo test", 6, 2);
            seed_bash("nudgeproj", "git commit", 6, 2);
            let mut log = Vec::new();
            run_learning_cycle(root, &CycleOptions::default(), &mut log);

            // Template-state skill -> the SessionStart nudge is present.
            let nudge = project_synthesis_nudge(root, "/work/nudgeproj");
            assert!(nudge.contains("learned-nudgeproj"), "nudge: {nudge}");
            assert!(
                nudge.contains("autonomous"),
                "nudge frames it as self-driven"
            );

            // The agent refines the skill -> the nudge self-clears.
            let skill_path = skills_directory(root)
                .join("learned-nudgeproj")
                .join("SKILL.md");
            fs::write(
                &skill_path,
                "---\nname: learned-nudgeproj\ngenerated: true\nprovenance: learned\n---\nRefined.\n",
            )
            .expect("refine");
            assert!(
                project_synthesis_nudge(root, "/work/nudgeproj").is_empty(),
                "nudge must disappear once the skill is refined"
            );
        });
    }

    #[test]
    fn synthesis_nudge_empty_for_project_without_generated_skill() {
        isolated_home("synth-nudge-empty", |root| {
            assert!(project_synthesis_nudge(root, "/work/noskill").is_empty());
        });
    }
}
