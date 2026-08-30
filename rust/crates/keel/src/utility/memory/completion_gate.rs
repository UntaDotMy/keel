//! Purpose: Working-brief completion-gate check command handler
//! Caller: mod.rs run_memory_command
//! Dependencies: crate::args::FlagSet, crate::json, crate::runtime, crate::utility::working_brief
//! Main Functions: run_completion_gate_command
//! Side Effects: `--proof` persists the proof text onto the brief record itself.
//!
//! The gate requires a named brief, at least one acceptance criterion, and
//! non-empty completion proof. A supplied `--proof` is written into the brief's
//! `proof` field via the working_brief storage APIs; later checks may reuse that
//! persisted proof.

use std::io::Write;

use crate::args::FlagSet;
use crate::json::Value;
use crate::runtime::resolve_claude_home;
use crate::utility::working_brief::{list_briefs, read_brief, write_brief};

use super::shared::{is_help_argument, probe_marker, probe_value, render_workflow_json};

pub(super) fn run_completion_gate_command(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: keel {command_group} completion-gate check [--brief-id <id>] [--proof <text>] [--claude-home <path>] [--json]"
        );
        let _ = writeln!(
            standard_output,
            "  Without --brief-id the available brief ids are listed instead of checking."
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "check" => run_completion_gate_check(
            command_group,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        other => {
            let _ = writeln!(
                standard_error,
                "Unknown {command_group} completion-gate action: {other} (expected check)"
            );
            1
        }
    }
}

fn run_completion_gate_check(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("completion-gate check");
    flag_set.string_flag("brief-id", "");
    flag_set.string_flag("proof", "");
    flag_set.string_flag("claude-home", "");
    flag_set.bool_flag("json", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "{command_group} completion-gate check: {error}"
            );
            return 1;
        }
    };

    // Brief probe: the gate condition. Without --brief-id, fail with the list
    // of available ids so the caller can pick one instead of guessing.
    let brief_id_input = flag_set.string_value("brief-id").trim().to_string();
    let brief_probe: Result<crate::utility::working_brief::Brief, String> =
        if brief_id_input.is_empty() {
            match list_briefs(&claude_home) {
                Ok(briefs) if briefs.is_empty() => Err(
                    "no --brief-id given and no working briefs exist; write one with \
                     `keel memory working-brief write` first"
                        .to_string(),
                ),
                Ok(briefs) => Err(format!(
                    "no --brief-id given; available brief ids: {}",
                    briefs
                        .iter()
                        .map(|brief| brief.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
                Err(error) => Err(error.to_string()),
            }
        } else {
            match read_brief(&claude_home, &brief_id_input) {
                Ok(Some(brief)) => Ok(brief),
                Ok(None) => Err(format!("no working brief with id {brief_id_input}")),
                Err(error) => Err(error.to_string()),
            }
        };
    let brief_status = match &brief_probe {
        Ok(brief) => format!("{} ({})", brief.id, brief.request),
        Err(error) => error.clone(),
    };

    let acceptance_probe: Result<String, String> = match &brief_probe {
        Ok(brief)
            if brief
                .acceptance_criteria
                .iter()
                .any(|criterion| !criterion.trim().is_empty()) =>
        {
            Ok(format!(
                "{} acceptance criterion/criteria recorded",
                brief
                    .acceptance_criteria
                    .iter()
                    .filter(|criterion| !criterion.trim().is_empty())
                    .count()
            ))
        }
        Ok(_) => Err("working brief has no acceptance criteria".to_string()),
        Err(_) => Err("acceptance criteria cannot be checked without a brief".to_string()),
    };
    let acceptance_status = acceptance_probe
        .as_ref()
        .map_or_else(Clone::clone, Clone::clone);

    // Proof is mandatory. A new value replaces the persisted proof; otherwise
    // the existing proof is reused so verification can be repeated safely.
    let raw_proof = flag_set.string_value("proof");
    let proof_input = raw_proof.trim().to_string();
    let proof_probe: Result<String, String> = if raw_proof.is_empty() {
        brief_probe
            .as_ref()
            .ok()
            .map(|brief| brief.proof.trim().to_string())
            .filter(|proof| !proof.is_empty())
            .ok_or_else(|| {
                "completion proof is required; pass --proof or record proof on the brief"
                    .to_string()
            })
    } else if proof_input.is_empty() {
        Err("proof argument is whitespace only".to_string())
    } else {
        Ok(proof_input.clone())
    };
    let proof_status = proof_probe.as_ref().map_or_else(Clone::clone, Clone::clone);

    // Persist the proof on the brief; failure fails the check.
    // A claimed proof must never be silently dropped.
    let persisted_probe: Option<Result<String, String>> = match (&brief_probe, &proof_probe) {
        (Ok(brief), Ok(proof)) if !raw_proof.is_empty() => {
            let mut updated = brief.clone();
            updated.proof = proof.clone();
            Some(match write_brief(&claude_home, &updated) {
                Ok(_) => Ok(format!("recorded on brief {}", updated.id)),
                Err(error) => Err(error.to_string()),
            })
        }
        _ => None,
    };
    let persisted_status = persisted_probe.as_ref().map(|probe| match probe {
        Ok(detail) => detail.clone(),
        Err(error) => error.clone(),
    });

    let all_ok = brief_probe.is_ok()
        && acceptance_probe.is_ok()
        && proof_probe.is_ok()
        && persisted_probe.as_ref().map(Result::is_ok).unwrap_or(true);

    if flag_set.bool_value("json") {
        let mut fields: Vec<(String, Value)> = vec![
            ("ok".into(), Value::Bool(all_ok)),
            (
                "workingBrief".into(),
                probe_value(&brief_probe, &brief_status),
            ),
            (
                "acceptanceCriteria".into(),
                probe_value(&acceptance_probe, &acceptance_status),
            ),
            ("proof".into(), probe_value(&proof_probe, &proof_status)),
            ("closureReady".into(), Value::Bool(all_ok)),
        ];
        if let (Some(probe), Some(status)) = (&persisted_probe, &persisted_status) {
            fields.push(("proofPersisted".into(), probe_value(probe, status)));
        }
        let exit = render_workflow_json(standard_output, standard_error, &Value::Object(fields));
        return if all_ok { exit } else { 1 };
    }

    let _ = writeln!(
        standard_output,
        "{command_group} completion-gate check: brief={brief_id_input} status={}",
        if all_ok { "ok" } else { "fail" }
    );
    let _ = writeln!(
        standard_output,
        "  working-brief: {} -> {brief_status}",
        probe_marker(&brief_probe)
    );
    let _ = writeln!(
        standard_output,
        "  acceptance-criteria: {} -> {acceptance_status}",
        probe_marker(&acceptance_probe)
    );
    let _ = writeln!(
        standard_output,
        "  proof: {} -> {proof_status}",
        probe_marker(&proof_probe)
    );
    if let (Some(probe), Some(status)) = (&persisted_probe, &persisted_status) {
        let _ = writeln!(
            standard_output,
            "  proof-persisted: {} -> {status}",
            probe_marker(probe)
        );
    }
    if !all_ok {
        let _ = writeln!(
            standard_output,
            "  hint: resolve the failing probes above, then re-run with --proof \"...\""
        );
        return 1;
    }
    let _ = writeln!(
        standard_output,
        "  hint: brief verified — close out the task via `keel review pre-pr`"
    );
    0
}
