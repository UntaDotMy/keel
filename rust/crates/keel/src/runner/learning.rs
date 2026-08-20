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
use std::path::{Path, PathBuf};

use crate::args::FlagSet;
use crate::runner::observation::{self, Observation};
use crate::runtime::{
    agents_directory, display_path, resolve_claude_home, skills_directory, write_text,
};
use crate::utility::record_store::{field, Record, RecordStore};

/// Rolling window of observation history the cycle distills each run. Older
/// signal naturally ages out of the window, giving confidence an implicit decay
/// without a separate decay pass: a habit the user dropped stops being counted.
const OBSERVE_WINDOW_DAYS: u64 = 14;

/// Minimum observations of a signature within the window before it is worth
/// recording as an instinct at all. Below this it is a one-off, not a pattern.
const INSTINCT_MIN_COUNT: u64 = 3;

/// Confidence ceiling so a single hyperactive day cannot dominate. Confidence is
/// an integer (consistent with the manual `memory instincts` store) equal to the
/// windowed observation count, clamped here.
const INSTINCT_CONFIDENCE_CAP: i64 = 20;

/// Base confidence bar for a signature to contribute to a generated skill.
const SKILL_MIN_CONFIDENCE: i64 = 4;

/// Preferred bar: seen across this many sessions (durable habit).
const SKILL_MIN_SESSIONS: usize = 2;

/// Strong single-session bar: enough volume in one long sitting can still promote
/// when multi-session evidence is not yet available (common on a new machine).
const SKILL_SINGLE_SESSION_CONFIDENCE: i64 = 8;

/// A project needs at least this many trusted instincts before a skill is worth
/// generating — one lone instinct does not justify a whole skill file.
const SKILL_MIN_INSTINCTS: usize = 2;

/// Auto-learned instincts whose confidence has decayed to this floor (their
/// pattern has aged out of every observation window) are pruned so the store
/// reflects current behavior rather than an ever-growing archive. Manual
/// instincts (`source != observed`) are never pruned regardless of confidence.
const INSTINCT_PRUNE_FLOOR: i64 = 0;

/// Group path for the shared instinct store, matching the manual
/// `keel memory instincts` surface so auto-learned and hand-authored
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
    /// A2: generated skills rolled back because their promotion prediction was
    /// falsified — the behavior that justified the skill no longer recurs at the
    /// trust bar, and the skill was still at its template state (never a manual
    /// edit). Removing a wrong skill is the empirical-falsification half of the
    /// evidence→prediction→falsify discipline.
    pub skills_rolled_back: usize,
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

    let observations = match observation::iter_recent_rows_at(claude_home, options.window_days) {
        Ok(rows) => rows,
        Err(error) => {
            let _ = writeln!(log, "keel learn: read observations failed: {error}");
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
                let _ = writeln!(log, "keel learn: write instinct failed: {error}");
                continue;
            }
        }
        report.instincts_recorded += 1;

        if is_trusted_habit(confidence, cluster.distinct_sessions) {
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
                let _ = writeln!(log, "keel learn: {message}");
            }
        }
    }

    // 4. A2 — evaluate every generated skill's falsifiable prediction and roll
    // back the ones whose justifying behavior no longer holds. This runs after
    // instincts were refreshed and decayed (step 2/2b), so the trust check sees
    // current behavior. Skipped on a dry run (it mutates disk).
    if !options.dry_run {
        report.skills_rolled_back = evaluate_predictions_and_rollback(claude_home, log);
    }

    report
}

const CONTINUOUS_LEARNING_INTERVAL: usize = 3;
const CONTINUOUS_LEARNING_WINDOW_DAYS: u64 = OBSERVE_WINDOW_DAYS;

/// Run learning from the PostToolUse path after a small batch of new signals.
///
/// SessionEnd remains a final reconciliation point, but learning must not stay
/// invisible until a host emits SessionEnd. The marker is scoped to the same
/// keel home as the observations and derived artifacts, so an override home
/// cannot consume or mutate another home's learning state.
pub fn run_continuous_learning_if_due(claude_home: &Path, log: &mut dyn std::io::Write) {
    if std::env::var("CLAUDE_SKILLS_LEARNING")
        .map(|value| value.trim().eq_ignore_ascii_case("off"))
        .unwrap_or(false)
    {
        return;
    }
    let observations =
        match observation::iter_recent_rows_at(claude_home, CONTINUOUS_LEARNING_WINDOW_DAYS) {
            Ok(rows) => rows,
            Err(error) => {
                let _ = writeln!(log, "keel learn: continuous read failed: {error}");
                return;
            }
        };
    let current_count = observations.len();
    if current_count == 0 {
        return;
    }

    let state_directory = claude_home.join("state").join("learning");
    let marker_path = state_directory.join("last-observation-count");
    let lock_path = state_directory.join("cycle.lock");
    let previous_count = fs::read_to_string(&marker_path)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    if current_count < previous_count.saturating_add(CONTINUOUS_LEARNING_INTERVAL) {
        return;
    }

    if fs::create_dir_all(&state_directory).is_err() {
        return;
    }
    if let Ok(metadata) = fs::metadata(&lock_path) {
        if metadata
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .map(|age| age > std::time::Duration::from_secs(300))
            .unwrap_or(false)
        {
            let _ = fs::remove_file(&lock_path);
        }
    }
    let Ok(lock) = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
    else {
        return;
    };

    let report = run_learning_cycle(claude_home, &CycleOptions::default(), log);
    if let Err(error) = write_text(&marker_path, &current_count.to_string()) {
        let _ = writeln!(log, "keel learn: continuous marker write failed: {error}");
    }
    drop(lock);
    let _ = fs::remove_file(&lock_path);
    if report.instincts_recorded > 0
        || report.skills_generated > 0
        || report.agents_generated > 0
        || report.skills_rolled_back > 0
    {
        let _ = writeln!(
            log,
            "keel learn: continuous cycle observations={} instincts={} skills={} agents={} rolled_back={}",
            current_count,
            report.instincts_recorded,
            report.skills_generated,
            report.agents_generated,
            report.skills_rolled_back
        );
    }
}

/// A2: empirical falsification of generated-skill predictions.
///
/// For every loop-generated skill, re-check the prediction recorded at promotion
/// time (the signatures that justified it). A skill is rolled back — its skill
/// directory and paired generated agent removed — only when ALL of these hold:
/// - the marker carries a non-empty prediction (pre-A2 markers are never touched);
/// - the on-disk SKILL.md is still byte-identical to what the loop generated (a
///   skill the agent manually refined is respected, exactly like the no-clobber
///   guard everywhere else in this module);
/// - the project no longer sustains enough of the predicted signatures at the
///   trust bar (confidence >= SKILL_MIN_CONFIDENCE across >= 2 sessions) to meet
///   `SKILL_MIN_INSTINCTS` — i.e. the prediction "this behavior will keep
///   happening" is falsified.
///
/// Returns the number of skills rolled back. Fail-open: any per-skill error logs
/// and continues so one bad skill cannot abort the sweep or the SessionEnd hook.
fn evaluate_predictions_and_rollback(claude_home: &Path, log: &mut dyn std::io::Write) -> usize {
    let skills_root = skills_directory(claude_home);
    let Ok(entries) = fs::read_dir(&skills_root) else {
        return 0;
    };
    let store = RecordStore::new(claude_home, INSTINCT_GROUP);
    let records = store.list_records().unwrap_or_default();

    let mut rolled_back = 0usize;
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(marker) = read_marker(&dir.join(LEARNING_META_FILE)) else {
            continue; // not a loop-generated skill
        };
        // No prediction recorded (pre-A2 marker): nothing to falsify.
        if marker.predicted_signatures.is_empty() || marker.project.trim().is_empty() {
            continue;
        }
        // Respect a manual refinement: a skill whose content the agent changed is
        // no longer a template and must never be auto-removed. why: this is an
        // intentional adoption semantic — the falsification rollback below only
        // governs template-state generated skills; #23 (git-root project bucketing)
        // is what prevents a wrong-project skill from being generated in the first
        // place, so the rollback net does not need to override adoption.
        let on_disk = fs::read(dir.join("SKILL.md")).unwrap_or_default();
        if fnv1a_64(&on_disk) != marker.generated_hash {
            continue;
        }

        // Count how many predicted signatures still hold at the trust bar.
        let still_trusted = count_trusted_predicted_signatures(
            &records,
            &marker.project,
            &marker.predicted_signatures,
        );
        // Prediction stands while the project still justifies a skill.
        if still_trusted >= SKILL_MIN_INSTINCTS {
            continue;
        }

        // Falsified: remove the skill directory and its paired generated agent.
        let skill_name = entry.file_name().to_string_lossy().to_string();
        match fs::remove_dir_all(&dir) {
            Ok(_) => {
                rolled_back += 1;
                let agent_path = agents_directory(claude_home).join(format!("{skill_name}.md"));
                if agent_path.is_file() {
                    let body = fs::read_to_string(&agent_path).unwrap_or_default();
                    if body.contains("generated: true") {
                        let _ = fs::remove_file(&agent_path);
                    }
                }
                let _ = writeln!(
                    log,
                    "keel learn: rolled back {skill_name} (prediction falsified: \
                     {still_trusted}/{} predicted signatures still trusted)",
                    marker.predicted_signatures.len()
                );
            }
            Err(error) => {
                let _ = writeln!(log, "keel learn: rollback {skill_name} failed: {error}");
            }
        }
    }
    rolled_back
}

/// Count how many of `predicted` signatures are still trusted for `project` in the
/// instinct store (confidence >= SKILL_MIN_CONFIDENCE AND seen across >= 2
/// sessions) — the same trust bar `evolve_skill` used to promote them.
fn count_trusted_predicted_signatures(
    records: &[(String, Record)],
    project: &str,
    predicted: &[String],
) -> usize {
    let predicted_set: std::collections::BTreeSet<&str> =
        predicted.iter().map(String::as_str).collect();
    let mut trusted = std::collections::BTreeSet::new();
    for (_, record) in records {
        if field(record, "project") != Some(project) {
            continue;
        }
        let trigger = field(record, "trigger").unwrap_or("");
        if !predicted_set.contains(trigger) {
            continue;
        }
        let confidence: i64 = field(record, "confidence")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let sessions: usize = field(record, "sessions")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        if is_trusted_habit(confidence, sessions) {
            trusted.insert(trigger.to_string());
        }
    }
    trusted.len()
}

/// Whether a signature is trusted enough to promote into a skill (or keep one).
/// Multi-session recurrence is preferred; strong single-session volume also qualifies
/// so learning works on a fresh machine without waiting for many calendar days.
fn is_trusted_habit(confidence: i64, distinct_sessions: usize) -> bool {
    if confidence < SKILL_MIN_CONFIDENCE {
        return false;
    }
    if distinct_sessions >= SKILL_MIN_SESSIONS {
        return true;
    }
    confidence >= SKILL_SINGLE_SESSION_CONFIDENCE
}

/// Render a compact, always-on digest of the trusted instincts for the project
/// rooted at `cwd`, for injection into SessionStart context. Empty string when
/// there is nothing trusted to surface, so the caller can append unconditionally
/// without adding a blank section.
///
/// This is the lightweight, always-on tier (ECC's key move): generated skills
/// are loaded on demand by the harness's matcher, but a one-line-per-instinct
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
        "Learned conventions for the {project} project (observed by keel; treat as defaults, not constraints):\n{}",
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

/// CLI surface: `keel learn [status|run|dry-run|synthesize] [--window N] [--json]`.
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
            "Usage: keel learn [status|run|dry-run|synthesize] [--window <days>] [--synthesize] [--json]\n\
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
                    "skillsRolledBack": report.skills_rolled_back,
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
                    "learn {action}: {verb} {} instinct(s); {} skill(s) generated, {} respected, {} rolled back, {} agent(s) generated",
                    report.instincts_recorded,
                    report.skills_generated,
                    report.skills_respected,
                    report.skills_rolled_back,
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
    let observations =
        observation::iter_recent_rows_at(claude_home, window_days).unwrap_or_default();
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
    let continuous_enabled = !std::env::var("CLAUDE_SKILLS_LEARNING")
        .map(|value| value.trim().eq_ignore_ascii_case("off"))
        .unwrap_or(false);
    let marker_path = claude_home
        .join("state")
        .join("learning")
        .join("last-observation-count");
    let last_continuous_count = fs::read_to_string(&marker_path)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);

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
            "continuous": {
                "enabled": continuous_enabled,
                "trigger": "PostToolUse + SessionEnd",
                "interval": CONTINUOUS_LEARNING_INTERVAL,
                "lastObservationCount": last_continuous_count,
                "marker": display_path(&marker_path),
            },
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
        let _ = writeln!(
            standard_output,
            "  continuous learning: {} via PostToolUse + SessionEnd; cycle every {} new observation(s); last cycle count={}",
            if continuous_enabled { "on" } else { "off" },
            CONTINUOUS_LEARNING_INTERVAL,
            last_continuous_count
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
        let Some(signature) = normalize_learning_signature(&observation.signature) else {
            continue; // noise (quote pollution, pure navigation) — never learn
        };
        let project = project_name(&observation.cwd);
        let key = format!("{project}\u{1f}{signature}");
        let cluster = clusters.entry(key.clone()).or_insert_with(|| Cluster {
            project: project.clone(),
            signature: signature.clone(),
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

/// Normalize and filter signatures before learning so polluted Windows wrappers
/// (`keel.exe'`) and pure navigation noise never become "conventions".
fn normalize_learning_signature(raw: &str) -> Option<String> {
    let failed = raw.ends_with(crate::runner::observation::FAILURE_SIGNATURE_SUFFIX);
    let base = raw
        .strip_suffix(crate::runner::observation::FAILURE_SIGNATURE_SUFFIX)
        .unwrap_or(raw)
        .trim()
        .trim_matches(['\'', '"', '`', '&', ' ']);
    if base.is_empty() {
        return None;
    }
    // Drop trailing quote pollution left by older PowerShell rewrite forms.
    let mut cleaned = base.trim_end_matches(['\'', '"']).to_string();
    // Strip Windows executable suffixes for stable program names.
    let lower = cleaned.to_ascii_lowercase();
    if let Some(stem) = lower
        .strip_suffix(".exe")
        .or_else(|| lower.strip_suffix(".cmd"))
        .or_else(|| lower.strip_suffix(".bat"))
        .or_else(|| lower.strip_suffix(".ps1"))
    {
        // Preserve path basename only when the whole token was a path to the binary.
        let stem = stem.rsplit(['/', '\\']).next().unwrap_or(stem);
        cleaned = stem.to_string();
    }
    if is_noise_learning_signature(&cleaned) {
        return None;
    }
    if failed {
        Some(format!(
            "{cleaned}{}",
            crate::runner::observation::FAILURE_SIGNATURE_SUFFIX
        ))
    } else {
        Some(cleaned)
    }
}

fn is_noise_learning_signature(signature: &str) -> bool {
    matches!(
        signature,
        "cd" | "echo"
            | "pwd"
            | "ls"
            | "dir"
            | "clear"
            | "cls"
            | "true"
            | "false"
            | "wc"
            | "cat"
            | "type"
            | "head"
            | "tail"
            | "which"
            | "where"
            | "bash"
            | "sh"
            | "zsh"
            | "cmd"
            | "powershell"
            | "pwsh"
    )
}

/// Derive a project name from an absolute cwd. The last path component is a
/// stable, human-meaningful label on a single-user machine. Empty cwd (older
/// hook inputs that omit it) maps to a shared `global` bucket.
fn project_name(cwd: &str) -> String {
    let trimmed = cwd.trim().trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return "global".to_string();
    }
    // why: bucket learning by the project ROOT, not the launch subdir. A session
    // started from `repo/rust` must learn into `repo`, not a separate `rust`
    // bucket that fragments the signal and collides with any other repo also
    // launched from a dir named `rust`. Walk up to the nearest ancestor holding a
    // `.git` entry (the repo root) and use its directory name.
    if let Some(root) = git_root_from(Path::new(trimmed)) {
        if let Some(name) = root
            .file_name()
            .and_then(|segment| segment.to_str())
            .filter(|segment| !segment.is_empty())
        {
            // why: two repos can share a root dir name (~/work/app vs ~/oss/app);
            // the path hash keeps their buckets, skills, and instincts distinct.
            let path_hash = fnv1a_64(root.to_string_lossy().as_bytes()) as u32;
            return format!("{name}-{path_hash:08x}");
        }
    }
    // Fallback: last path segment (non-git trees, or synthetic paths in tests).
    trimmed
        .rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or("global")
        .to_string()
}

/// Nearest ancestor of `start` (inclusive) that contains a `.git` entry — the git
/// repository root. `None` when `start` is not inside a git repository.
fn git_root_from(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
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
    // A recurring FAILURE is a caution (outcome learning), not a habit to repeat.
    if let Some(base) = cluster
        .signature
        .strip_suffix(crate::runner::observation::FAILURE_SIGNATURE_SUFFIX)
    {
        return format!(
            "OUTCOME/WATCHOUT: `{base}` fails often here — diagnose before relying on a green run"
        );
    }
    match cluster.tool_name.as_str() {
        "Bash" => format!("PROCEDURE: {}", bash_procedure_line(&cluster.signature)),
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            let extension = cluster.signature.strip_prefix("edit:").unwrap_or("files");
            format!("PROCEDURE: primary work surfaces are `.{extension}` files in this project")
        }
        _ => format!(
            "PROCEDURE: repeated action `{sig}` — prefer this path unless the task says otherwise",
            sig = cluster.signature
        ),
    }
}

/// Map a Bash signature to a role-aware procedure line instead of labeling every
/// shell habit as "verifying or building" (git commit is not a verify step).
fn bash_procedure_line(signature: &str) -> String {
    let sig = signature.trim();
    let lower = sig.to_ascii_lowercase();
    let role = if lower.starts_with("git commit")
        || lower.starts_with("git push")
        || lower.starts_with("git pull")
        || lower.starts_with("git merge")
        || lower.starts_with("git rebase")
        || lower.starts_with("git checkout")
        || lower.starts_with("git switch")
        || lower.starts_with("git stash")
        || lower.starts_with("git add")
        || lower.starts_with("git restore")
        || lower.starts_with("git reset")
        || lower.starts_with("git branch")
        || lower.starts_with("git status")
        || lower.starts_with("git diff")
        || lower.starts_with("git log")
        || lower.starts_with("gh ")
    {
        "when doing version-control work in this project"
    } else if lower.starts_with("cargo test")
        || lower.starts_with("cargo check")
        || lower.starts_with("cargo clippy")
        || lower.starts_with("cargo fmt")
        || lower.starts_with("cargo build")
        || lower.starts_with("npm test")
        || lower.starts_with("npm run test")
        || lower.starts_with("pnpm test")
        || lower.starts_with("yarn test")
        || lower.starts_with("pytest")
        || lower.starts_with("go test")
        || lower.contains(" test")
        || lower.ends_with(" test")
    {
        "when verifying or building in this project"
    } else if lower.starts_with("docker")
        || lower.starts_with("kubectl")
        || lower.starts_with("helm")
        || lower.starts_with("terraform")
        || lower.starts_with("pulumi")
    {
        "when operating infrastructure in this project"
    } else {
        "when working in this project"
    };
    format!("{role}, run `{sig}`")
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
            let _ = writeln!(log, "keel learn: list instincts failed: {error}");
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
                    let _ = writeln!(log, "keel learn: prune instinct failed: {error}");
                }
            }
        } else {
            if let Some(slot) = record.iter_mut().find(|(key, _)| key == "confidence") {
                slot.1 = decayed.to_string();
            } else {
                record.push(("confidence".to_string(), decayed.to_string()));
            }
            if let Err(error) = store.write_record(&id, &record) {
                let _ = writeln!(log, "keel learn: decay instinct failed: {error}");
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
    // A2: the prediction is exactly the signatures justifying this skill. If the
    // project later stops sustaining enough of these at the trust bar, the
    // prediction is falsified and the skill is rolled back.
    let predicted_signatures = predicted_signatures(instincts);
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
        let _ = writeln!(log, "keel learn: would generate {skill_name}");
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
    if let Err(error) = write_marker(
        &meta_path,
        content_hash,
        &signature_set,
        project,
        &predicted_signatures,
    ) {
        return EvolveOutcome::Failed(error);
    }

    // Scaffold a subagent that preloads the new skill, marked generated.
    let agent_generated = match generate_agent(claude_home, &skill_name, project, log) {
        Ok(generated) => generated,
        Err(error) => {
            let _ = writeln!(log, "keel learn: {error}");
            false
        }
    };

    EvolveOutcome::Generated { agent_generated }
}

/// Build the synthesis brief for one generated skill: a concrete, ready-to-use
/// instruction the session agent fulfils to replace the deterministic template
/// prose with richer, project-specific prose. The binary never calls an LLM —
/// this is the bridge that lets the *session* model (which the harness already
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
         It currently holds a deterministic template. Rewrite it into a real project skill for \
         `{project}` using ONLY these observed conventions as the source of truth:\n{observed}\
         Requirements:\n\
         - Keep YAML frontmatter intact (name, description, when_to_use, generated: true, \
         provenance: learned must remain; improve description/when_to_use wording only).\n\
         - Structure body as: ## Procedures (do this), ## Watchouts (failed outcomes), \
         ## Operating rules.\n\
         - Write concrete \"when X, do Y\" steps — not summaries of the list above.\n\
         - Separate successful commands from failure watchouts.\n\
         - Do not invent commands, files, frameworks, or APIs not implied by the observations.\n\
         - Do not invent features outside what the observations support.\n\
         Your edit is protected — the learning loop detects content-hash changes and will not \
         overwrite it.",
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

/// A2: the sorted, de-duplicated signatures recorded as a skill's falsifiable
/// prediction at promotion time. Same content as `signature_set` but as a list,
/// so the marker can store and a later cycle can re-check each one individually.
fn predicted_signatures(instincts: &[TrustedInstinct]) -> Vec<String> {
    let mut signatures: Vec<String> = instincts.iter().map(|i| i.signature.clone()).collect();
    signatures.sort();
    signatures.dedup();
    signatures
}

/// Render the deterministic SKILL.md body for a project's trusted instincts.
/// Action-oriented: procedures and watchouts first so a matcher-loaded skill
/// changes agent behavior, not only lists statistics.
fn render_skill(skill_name: &str, project: &str, instincts: &[TrustedInstinct]) -> String {
    let mut procedures: Vec<&TrustedInstinct> = Vec::new();
    let mut watchouts: Vec<&TrustedInstinct> = Vec::new();
    for instinct in instincts {
        if instinct
            .signature
            .ends_with(crate::runner::observation::FAILURE_SIGNATURE_SUFFIX)
            || instinct.guidance.contains("WATCHOUT")
        {
            watchouts.push(instinct);
        } else {
            procedures.push(instinct);
        }
    }

    let mut body = String::new();
    body.push_str("---\n");
    body.push_str(&format!("name: {skill_name}\n"));
    body.push_str(&format!(
        "description: Learned procedures for the {project} project from observed command and edit patterns. Prefer these defaults before inventing a new workflow.\n"
    ));
    body.push_str(&format!(
        "when_to_use: When working in the {project} project (or a path under it) — apply learned procedures and watchouts instead of re-deriving the local workflow from scratch.\n"
    ));
    body.push_str("generated: true\n");
    body.push_str("generator: keel-learning\n");
    body.push_str("provenance: learned\n");
    body.push_str("---\n\n");
    body.push_str(&format!("# Learned workflow: {project}\n\n"));
    body.push_str(
        "Auto-generated from repeated actions in this project. Safe to refine: the learning loop \
detects content-hash changes and will not overwrite your edits. Built-in skills are never modified.\n\n",
    );

    body.push_str("## Procedures (do this)\n\n");
    if procedures.is_empty() {
        body.push_str("- (no successful recurring procedures yet)\n");
    } else {
        for instinct in &procedures {
            let line = instinct
                .guidance
                .strip_prefix("PROCEDURE: ")
                .unwrap_or(instinct.guidance.as_str());
            body.push_str(&format!(
                "- {line} (evidence: {}× / {} session(s), confidence {})\n",
                instinct.count, instinct.distinct_sessions, instinct.confidence
            ));
        }
    }

    body.push_str("\n## Watchouts (outcomes that failed)\n\n");
    if watchouts.is_empty() {
        body.push_str("- (no recurring failure patterns recorded)\n");
    } else {
        for instinct in &watchouts {
            let line = instinct
                .guidance
                .strip_prefix("OUTCOME/WATCHOUT: ")
                .unwrap_or(instinct.guidance.as_str());
            body.push_str(&format!(
                "- {line} (evidence: {}× / {} session(s), confidence {})\n",
                instinct.count, instinct.distinct_sessions, instinct.confidence
            ));
        }
    }

    body.push_str(
        "\n## Operating rules\n\n\
- Prefer the procedures above as defaults for this project.\n\
- Treat watchouts as pre-flight checks before relying on those commands.\n\
- If the user request conflicts with a learned default, follow the user request.\n\
- Do not invent extra workflow steps that are not listed here or asked for.\n",
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
        "description: Works within the {project} project using its learned conventions. Auto-generated by keel.\n"
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
    let _ = writeln!(log, "keel learn: generated agent {skill_name}");
    Ok(true)
}

/// Sidecar marker recording what the loop last generated, for the no-clobber guard.
struct LearningMarker {
    generated_hash: u64,
    signature_set: String,
    project: String,
    /// A2 (falsifiable prediction): the signatures whose trusted recurrence
    /// justified generating this skill, recorded at promotion time. A later cycle
    /// re-checks them — if the project no longer sustains enough of these at the
    /// trust bar, the prediction ("this behavior will keep happening") is
    /// falsified and the skill is rolled back. Empty for pre-A2 markers, which the
    /// evaluator treats as "no prediction to falsify" (never auto-rolled-back).
    predicted_signatures: Vec<String>,
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
    let predicted_signatures = value
        .get("predictedSignatures")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Some(LearningMarker {
        generated_hash: generated_hash.parse().ok()?,
        signature_set,
        project,
        predicted_signatures,
    })
}

fn write_marker(
    path: &Path,
    content_hash: u64,
    signature_set: &str,
    project: &str,
    predicted_signatures: &[String],
) -> Result<(), String> {
    let value = serde_json::json!({
        "generator": "keel-learning",
        "generatedHash": content_hash.to_string(),
        "signatureSet": signature_set,
        "project": project,
        "predictedSignatures": predicted_signatures,
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
        let distinct_sessions: usize = field(record, "sessions")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        if !is_trusted_habit(confidence, distinct_sessions) {
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
            "keel-learning-{}-{nanos}-{suffix}",
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
    fn continuous_cycle_runs_after_three_new_observations() {
        isolated_home("continuous", |root| {
            seed_bash("continuous-project", "cargo test", 3, 2);
            let mut log = Vec::new();
            run_continuous_learning_if_due(root, &mut log);

            let marker = root
                .join("state")
                .join("learning")
                .join("last-observation-count");
            assert_eq!(fs::read_to_string(&marker).expect("continuous marker"), "3");
            let store = RecordStore::new(root, INSTINCT_GROUP);
            assert!(
                !store.list_records().expect("list instincts").is_empty(),
                "continuous cycle must materialize an instinct"
            );

            run_continuous_learning_if_due(root, &mut log);
            assert_eq!(
                fs::read_to_string(&marker).expect("stable continuous marker"),
                "3"
            );
        });
    }

    #[test]
    fn bash_procedure_line_roles_by_signature_family() {
        assert!(bash_procedure_line("git commit").contains("version-control"));
        assert!(bash_procedure_line("cargo test").contains("verifying or building"));
        assert!(bash_procedure_line("docker compose up").contains("infrastructure"));
        assert!(bash_procedure_line("rg TODO").contains("when working in this project"));
        assert!(!bash_procedure_line("git commit").contains("verifying or building"));
    }

    #[test]
    fn project_name_takes_last_path_segment() {
        // Synthetic paths that are not inside a git repo fall back to the last
        // segment (the git-root walk finds no `.git` and yields None).
        assert_eq!(project_name("/work/myproj"), "myproj");
        assert_eq!(project_name("C:\\Users\\x\\repo\\"), "repo");
        assert_eq!(project_name(""), "global");
    }

    #[test]
    fn project_name_resolves_to_git_root_not_launch_subdir() {
        let base = std::env::temp_dir().join(format!("keel-proj-{}", std::process::id()));
        let repo = base.join("myrepo");
        let sub = repo.join("rust").join("crates");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        // A session launched from repo/rust/crates buckets into the repo root,
        // not the "crates"/"rust" subdir. The key carries the root dir name plus
        // a hash of the absolute root path.
        let key = project_name(&sub.to_string_lossy());
        assert!(key.starts_with("myrepo-"), "git-root key: {key}");
        assert_eq!(key.len(), "myrepo-".len() + 8);
        // Outside any git repo, still falls back to the last segment.
        let plain = base.join("plaindir");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(project_name(&plain.to_string_lossy()), "plaindir");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn project_name_disambiguates_same_named_roots_by_path() {
        let base = std::env::temp_dir().join(format!("keel-proj-collision-{}", std::process::id()));
        let work = base.join("work").join("app");
        let oss = base.join("oss").join("app");
        std::fs::create_dir_all(work.join(".git")).unwrap();
        std::fs::create_dir_all(oss.join(".git")).unwrap();
        let work_key = project_name(&work.to_string_lossy());
        let oss_key = project_name(&oss.to_string_lossy());
        assert_ne!(work_key, oss_key);
        assert!(work_key.starts_with("app-"), "name prefix kept: {work_key}");
        assert!(oss_key.starts_with("app-"), "name prefix kept: {oss_key}");
        // The same path hashes to the same key on every call.
        assert_eq!(work_key, project_name(&work.to_string_lossy()));
        assert_eq!(oss_key, project_name(&oss.to_string_lossy()));
        // Skill slugs and instinct ids diverge with the project key.
        assert_ne!(project_slug(&work_key), project_slug(&oss_key));
        assert_ne!(
            instinct_id(&work_key, "cargo test"),
            instinct_id(&oss_key, "cargo test")
        );
        let _ = std::fs::remove_dir_all(&base);
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
    fn recurring_failure_becomes_a_caution_instinct() {
        isolated_home("failure-instinct", |root| {
            // A command that fails repeatedly across two sessions clusters under a
            // distinct `… (failed)` signature and must surface as a caution, not a
            // "frequently runs" habit — the Reflexion-style learn-from-mistakes path.
            for index in 0..4 {
                let session = format!("s{}", index % 2);
                let input = json!({
                    "tool_name": "Bash",
                    "session_id": session,
                    "cwd": "/work/gamma",
                    "tool_input": { "command": "cargo test --workspace" },
                });
                observation::record_failure_observation(&input).expect("record failure");
            }
            let mut log = Vec::new();
            let report = run_learning_cycle(root, &CycleOptions::default(), &mut log);
            assert_eq!(report.instincts_recorded, 1);
            let store = RecordStore::new(root, INSTINCT_GROUP);
            let failed_sig = format!("cargo test{}", observation::FAILURE_SIGNATURE_SUFFIX);
            let id = instinct_id("gamma", &failed_sig);
            let record = store.read_record(&id).expect("read").expect("exists");
            let guidance = field(&record, "guidance").unwrap_or("");
            assert!(
                guidance.contains("WATCHOUT") || guidance.contains("fails often"),
                "failure instinct must read as a caution, got: {guidance}"
            );
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
    fn cycle_weak_single_session_does_not_promote_skill() {
        isolated_home("one-session", |root| {
            // Below single-session bar (conf 7 < 8) and only one session → instinct yes, skill no.
            seed_bash("delta", "cargo test", 7, 1);
            seed_bash("delta", "git commit", 7, 1);
            let mut log = Vec::new();
            let report = run_learning_cycle(root, &CycleOptions::default(), &mut log);
            assert_eq!(report.skills_generated, 0);
            assert!(report.instincts_recorded >= 2);
        });
    }

    #[test]
    fn cycle_strong_single_session_promotes_skill() {
        isolated_home("strong-one-session", |root| {
            // conf >= SKILL_SINGLE_SESSION_CONFIDENCE with one session still promotes
            // so a fresh machine can learn without waiting for a second day.
            seed_bash("strongdelta", "cargo test", 8, 1);
            seed_bash("strongdelta", "git commit", 8, 1);
            let mut log = Vec::new();
            let report = run_learning_cycle(root, &CycleOptions::default(), &mut log);
            assert_eq!(
                report.skills_generated, 1,
                "strong single-session habits promote: {:?}",
                report.notes
            );
            let body = fs::read_to_string(
                skills_directory(root)
                    .join("learned-strongdelta")
                    .join("SKILL.md"),
            )
            .expect("skill");
            assert!(body.contains("## Procedures (do this)"));
            assert!(body.contains("## Watchouts (outcomes that failed)"));
            assert!(body.contains("## Operating rules"));
        });
    }

    #[test]
    fn normalize_drops_noise_and_strips_exe_pollution() {
        assert_eq!(normalize_learning_signature("cd"), None);
        assert_eq!(normalize_learning_signature("ls"), None);
        assert_eq!(normalize_learning_signature("pwsh"), None);
        assert_eq!(
            normalize_learning_signature("keel.exe'").as_deref(),
            Some("keel")
        );
        assert_eq!(
            normalize_learning_signature("cargo test").as_deref(),
            Some("cargo test")
        );
        assert_eq!(
            normalize_learning_signature(&format!(
                "cargo test{}",
                observation::FAILURE_SIGNATURE_SUFFIX
            ))
            .as_deref(),
            Some("cargo test (failed)")
        );
    }

    #[test]
    fn noise_signatures_never_become_instincts() {
        isolated_home("noise", |root| {
            seed_bash("noisep", "cd", 10, 2);
            seed_bash("noisep", "ls -la", 10, 2);
            let mut log = Vec::new();
            let report = run_learning_cycle(root, &CycleOptions::default(), &mut log);
            assert_eq!(report.instincts_recorded, 0);
            assert_eq!(report.skills_generated, 0);
        });
    }

    #[test]
    fn skill_template_separates_procedures_and_failure_watchouts() {
        isolated_home("skill-sections", |root| {
            seed_bash("sect", "cargo test", 6, 2);
            seed_bash("sect", "git commit", 6, 2);
            for index in 0..4 {
                let session = format!("s{}", index % 2);
                let input = json!({
                    "tool_name": "Bash",
                    "session_id": session,
                    "cwd": "/work/sect",
                    "tool_input": { "command": "cargo clippy" },
                });
                observation::record_failure_observation(&input).expect("fail");
            }
            // Need the failure to reach trust as a third instinct (≥2 trusted).
            // conf 4 across 2 sessions is enough for multi-session trust.
            let mut log = Vec::new();
            let report = run_learning_cycle(root, &CycleOptions::default(), &mut log);
            assert_eq!(report.skills_generated, 1, "notes: {:?}", report.notes);
            let body =
                fs::read_to_string(skills_directory(root).join("learned-sect").join("SKILL.md"))
                    .expect("skill");
            assert!(body.contains("## Procedures (do this)"));
            assert!(body.contains("## Watchouts (outcomes that failed)"));
            assert!(
                body.contains("WATCHOUT") || body.contains("fails often"),
                "failure guidance in watchouts: {body}"
            );
            assert!(body.contains("cargo test") || body.contains("PROCEDURE"));
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

    // ---- A2: falsifiable prediction + rollback ----

    /// Write a template-state generated skill directory with a marker carrying a
    /// falsifiable prediction, mirroring exactly what `evolve_skill` writes. Used to
    /// test step-4 evaluation in isolation, without depending on observation-window
    /// timing (the real falsification path is multi-day decay, which a unit test
    /// cannot wait for).
    fn seed_generated_skill(root: &Path, project: &str, predicted: &[&str]) {
        let slug = project_slug(project);
        let skill_dir = skills_directory(root).join(format!("learned-{slug}"));
        fs::create_dir_all(&skill_dir).expect("mkdir skill");
        let content =
            format!("---\nname: learned-{slug}\ngenerated: true\nprovenance: learned\n---\nbody\n");
        fs::write(skill_dir.join("SKILL.md"), &content).expect("write skill");
        let marker = serde_json::json!({
            "generator": "keel-learning",
            "generatedHash": fnv1a_64(content.as_bytes()).to_string(),
            "signatureSet": predicted.join("\n"),
            "project": project,
            "predictedSignatures": predicted,
        });
        fs::write(
            skill_dir.join(LEARNING_META_FILE),
            serde_json::to_string_pretty(&marker).unwrap(),
        )
        .expect("write marker");
        // Pair a generated agent so rollback can verify it is removed too.
        let agent_path = agents_directory(root).join(format!("learned-{slug}.md"));
        fs::create_dir_all(agent_path.parent().unwrap()).expect("mkdir agents");
        fs::write(
            &agent_path,
            format!("---\nname: learned-{slug}\ngenerated: true\n---\nbody\n"),
        )
        .expect("write agent");
    }

    #[test]
    fn marker_records_predicted_signatures_at_promotion() {
        isolated_home("a2-marker", |root| {
            seed_bash("predproj", "cargo test", 6, 2);
            seed_bash("predproj", "git commit", 6, 2);
            let mut log = Vec::new();
            run_learning_cycle(root, &CycleOptions::default(), &mut log);

            let meta = skills_directory(root)
                .join("learned-predproj")
                .join(LEARNING_META_FILE);
            let marker = read_marker(&meta).expect("marker exists");
            assert_eq!(
                marker.predicted_signatures,
                vec!["cargo test".to_string(), "git commit".to_string()],
                "promotion records the justifying signatures as the prediction"
            );
        });
    }

    #[test]
    fn falsified_prediction_rolls_back_template_skill() {
        isolated_home("a2-rollback", |root| {
            // A previously-generated skill predicting two signatures, but the
            // instinct store no longer trusts ANY of them (the behavior stopped and
            // the instincts decayed away over prior cycles). The prediction is
            // therefore falsified.
            seed_generated_skill(root, "rbproj", &["cargo test", "git commit"]);
            assert!(skills_directory(root).join("learned-rbproj").exists());

            // Unrelated live signal so the cycle proceeds past the empty-observation
            // early return and reaches step 4.
            seed_bash("otherproj", "npm test", 4, 2);
            let mut log = Vec::new();
            let report = run_learning_cycle(root, &CycleOptions::default(), &mut log);
            assert_eq!(
                report.skills_rolled_back, 1,
                "a falsified prediction must roll the skill back"
            );
            assert_removed_eventually(&skills_directory(root).join("learned-rbproj"));
            assert_removed_eventually(&agents_directory(root).join("learned-rbproj.md"));
        });
    }

    #[test]
    fn rollback_respects_manually_refined_skill() {
        isolated_home("a2-respect", |root| {
            seed_generated_skill(root, "keepproj", &["cargo test", "git commit"]);
            // The agent refines the skill (hash now differs from the marker).
            let skill_path = skills_directory(root)
                .join("learned-keepproj")
                .join("SKILL.md");
            fs::write(
                &skill_path,
                "---\nname: learned-keepproj\ngenerated: true\nprovenance: learned\n---\nHand-authored.\n",
            )
            .expect("refine");

            // Prediction is falsified (no trusted instincts), but a manually-refined
            // skill is never auto-removed.
            seed_bash("liveproj", "npm test", 4, 2);
            let mut log = Vec::new();
            let report = run_learning_cycle(root, &CycleOptions::default(), &mut log);
            assert_eq!(
                report.skills_rolled_back, 0,
                "a manually-refined skill must survive a falsified prediction"
            );
            assert!(
                skill_path.exists(),
                "the refined skill file must still be present"
            );
            let body = fs::read_to_string(&skill_path).expect("read");
            assert!(body.contains("Hand-authored"), "manual prose preserved");
        });
    }

    #[test]
    fn still_trusted_prediction_keeps_skill() {
        isolated_home("a2-keep", |root| {
            seed_generated_skill(root, "steadyproj", &["cargo test", "git commit"]);
            // The behavior still holds: write trusted instincts for both predicted
            // signatures (confidence >= bar, >= 2 sessions). The skill must stay.
            let store = RecordStore::new(root, INSTINCT_GROUP);
            for trigger in ["cargo test", "git commit"] {
                let id = instinct_id("steadyproj", trigger);
                store
                    .write_record(
                        &id,
                        &vec![
                            ("id".into(), id.clone()),
                            ("trigger".into(), trigger.into()),
                            ("guidance".into(), "x".into()),
                            ("confidence".into(), "6".into()),
                            ("sessions".into(), "2".into()),
                            ("project".into(), "steadyproj".into()),
                            ("source".into(), SOURCE_OBSERVED.into()),
                        ],
                    )
                    .expect("seed trusted");
            }

            seed_bash("liveproj", "npm test", 4, 2);
            let mut log = Vec::new();
            let report = run_learning_cycle(root, &CycleOptions::default(), &mut log);
            assert_eq!(
                report.skills_rolled_back, 0,
                "a prediction that still holds must keep the skill"
            );
            assert!(skills_directory(root).join("learned-steadyproj").exists());
        });
    }

    #[test]
    fn pre_a2_marker_without_prediction_is_never_rolled_back() {
        isolated_home("a2-legacy", |root| {
            // Simulate a skill generated before A2: marker has no predictedSignatures.
            let skill_dir = skills_directory(root).join("learned-legacyproj");
            fs::create_dir_all(&skill_dir).expect("mkdir");
            let content = "---\nname: learned-legacyproj\ngenerated: true\n---\nbody\n";
            fs::write(skill_dir.join("SKILL.md"), content).expect("write skill");
            let legacy_marker = serde_json::json!({
                "generator": "keel-learning",
                "generatedHash": fnv1a_64(content.as_bytes()).to_string(),
                "signatureSet": "cargo test",
                "project": "legacyproj",
                // no predictedSignatures key
            });
            fs::write(
                skill_dir.join(LEARNING_META_FILE),
                serde_json::to_string_pretty(&legacy_marker).unwrap(),
            )
            .expect("write marker");

            // Some live signal so the cycle runs.
            seed_bash("liveproj", "npm test", 4, 2);
            let mut log = Vec::new();
            let report = run_learning_cycle(root, &CycleOptions::default(), &mut log);
            assert_eq!(
                report.skills_rolled_back, 0,
                "a pre-A2 marker with no prediction must never be auto-rolled-back"
            );
            assert!(skill_dir.exists(), "legacy generated skill is preserved");
        });
    }
}
