use std::io::Write;

use crate::args::FlagSet;
use crate::runtime::write_text;
use crate::utility::anvil::job;
use crate::utility::anvil::supervisor;

pub fn run_stamp(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("anvil stamp");
    flags.bool_flag("strict", false);
    flags.bool_flag("dry-run", false);
    flags.string_flag("piece", "");
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let _ = flags.string_value("piece");
    let dry_run = flags.bool_value("dry-run");
    let strict = flags.bool_value("strict");
    let paths = match job::JobPaths::resolve(
        flags.string_value("workspace-root"),
        flags.string_value("claude-home"),
    ) {
        Ok(value) => Some(value),
        Err(_) if dry_run => None,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let casts = paths
        .as_ref()
        .and_then(|value| list_cast_ids(value).ok())
        .unwrap_or_default();
    if dry_run {
        let _ = writeln!(
            standard_output,
            "anvil stamp: dry-run casts={} strict={} mode=ppt-evidence",
            casts.len(),
            strict
        );
        return 0;
    }
    if casts.is_empty() {
        let _ = writeln!(standard_error, "anvil stamp: no survivors to score");
        return 1;
    }
    let evidence = match load_cast_evidence(paths.as_ref().expect("paths for non-dry run")) {
        Ok(value) => value,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let winner = match pick_evidence_winner(&evidence, strict) {
        Ok(value) => value,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let winner_evidence = &evidence[winner];
    let value = paths.as_ref().expect("paths for non-dry run");
    if let Err(error) = promote_winner(value, winner_evidence) {
        let _ = writeln!(standard_error, "{error}");
        return 1;
    }
    let line = format!(
        "anvil stamp: winner={} gate_ok={} clipped_len={} strict={} mode=ppt-evidence",
        winner_evidence.id, winner_evidence.gate_ok, winner_evidence.clipped_len, strict
    );
    let _ = writeln!(standard_output, "{line}");
    let _ = writeln!(
        standard_output,
        "{}",
        supervisor::clip_output(
            &format!(
                "winner {} gate_ok={} clipped_len={}",
                winner_evidence.id, winner_evidence.gate_ok, winner_evidence.clipped_len
            ),
            4000
        )
    );
    0
}

fn list_cast_ids(paths: &job::JobPaths) -> Result<Vec<String>, String> {
    let dir = &paths.dir;
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut ids = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("cast_") && entry.path().join("result.json").is_file() {
            ids.push(name);
        }
    }
    ids.sort();
    Ok(ids)
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct CastEvidence {
    id: String,
    gate_ok: bool,
    clipped_len: usize,
}

fn load_cast_evidence(paths: &job::JobPaths) -> Result<Vec<CastEvidence>, String> {
    let ids = list_cast_ids(paths)?;
    if ids.is_empty() {
        return Err("anvil stamp: no cast evidence found".to_string());
    }
    ids.into_iter()
        .map(|id| {
            let path = paths.dir.join(&id).join("result.json");
            let text = std::fs::read_to_string(&path)
                .map_err(|error| format!("anvil stamp: read {id} evidence: {error}"))?;
            let value: serde_json::Value = serde_json::from_str(&text)
                .map_err(|error| format!("anvil stamp: parse {id} evidence: {error}"))?;
            let gate_ok = value
                .get("gate_ok")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| format!("anvil stamp: {id} evidence lacks gate_ok"))?;
            let clipped_len = value
                .get("clipped_len")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| format!("anvil stamp: {id} evidence lacks clipped_len"))?
                as usize;
            Ok(CastEvidence {
                id,
                gate_ok,
                clipped_len,
            })
        })
        .collect()
}
fn evidence_strength(evidence: &CastEvidence) -> f64 {
    let gate = if evidence.gate_ok { 1.0 } else { 0.0 };
    gate + 1.0 / (1.0 + evidence.clipped_len as f64)
}

fn bradley_terry_pref(left: f64, right: f64) -> f64 {
    1.0 / (1.0 + (-(left - right)).exp())
}

fn evidence_rank_better(candidate: &CastEvidence, incumbent: &CastEvidence) -> bool {
    (candidate.gate_ok && !incumbent.gate_ok)
        || (candidate.gate_ok == incumbent.gate_ok
            && (candidate.clipped_len < incumbent.clipped_len
                || (candidate.clipped_len == incumbent.clipped_len && candidate.id < incumbent.id)))
}

fn pick_evidence_winner(evidence: &[CastEvidence], strict: bool) -> Result<usize, String> {
    if evidence.is_empty() {
        return Err("anvil stamp: no cast evidence to rank".to_string());
    }
    let n = evidence.len();
    let mut winner = 0usize;
    if n >= 2 {
        let strengths: Vec<f64> = evidence.iter().map(evidence_strength).collect();
        let mut wins = vec![0.0; n];
        let mut counts = vec![0.0; n];
        for i in 0..n {
            let j = (i + 1) % n;
            let pref = bradley_terry_pref(strengths[i], strengths[j]);
            wins[i] += pref;
            wins[j] += 1.0 - pref;
            counts[i] += 1.0;
            counts[j] += 1.0;
        }
        let mut best_mean = f64::NEG_INFINITY;
        for i in 0..n {
            let mean = wins[i] / counts[i];
            let better = mean > best_mean + 1e-12
                || ((mean - best_mean).abs() <= 1e-12
                    && evidence_rank_better(&evidence[i], &evidence[winner]));
            if better {
                best_mean = mean;
                winner = i;
            }
        }
    }
    if strict && !evidence[winner].gate_ok {
        return Err("anvil stamp: --strict requires a passing survivor".to_string());
    }
    Ok(winner)
}

pub(crate) fn ensure_winner_workspace(
    paths: &job::JobPaths,
    strict: bool,
) -> Result<std::path::PathBuf, String> {
    let workspace = paths.out_dir().join("workspace");
    if workspace.is_dir() {
        return Ok(workspace);
    }
    let evidence = load_cast_evidence(paths).map_err(|error| {
        format!(
            "anvil loop: no promoted winner workspace at {}: {error}",
            workspace.display()
        )
    })?;
    let winner = pick_evidence_winner(&evidence, strict).map_err(|error| {
        format!(
            "anvil loop: no promoted winner workspace at {}: {error}",
            workspace.display()
        )
    })?;
    promote_winner(paths, &evidence[winner])?;
    if !workspace.is_dir() {
        return Err(format!(
            "anvil loop: no promoted winner workspace at {}",
            workspace.display()
        ));
    }
    Ok(workspace)
}

fn promote_winner(paths: &job::JobPaths, winner: &CastEvidence) -> Result<(), String> {
    let out = paths.out_dir();
    let result = paths.dir.join(&winner.id).join("result.json");
    if !result.is_file() {
        return Err(format!(
            "anvil stamp: missing cast evidence {}",
            result.display()
        ));
    }
    let result_text = std::fs::read_to_string(&result)
        .map_err(|error| format!("anvil stamp: read result: {error}"))?;
    let value: serde_json::Value = serde_json::from_str(&result_text)
        .map_err(|error| format!("anvil stamp: parse result: {error}"))?;
    let workspace = value
        .get("workspace")
        .and_then(|item| item.as_str())
        .ok_or_else(|| "anvil stamp: result lacks workspace".to_string())?;

    let staging = out.with_file_name(format!("anvil_out.tmp-{}", std::process::id()));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|error| error.to_string())?;
    }
    std::fs::create_dir_all(&staging).map_err(|error| error.to_string())?;
    if let Err(error) = write_text(&staging.join("result.json"), &result_text)
        .and_then(|_| {
            crate::utility::anvil::workspace::copy_tree(
                std::path::Path::new(workspace),
                &staging.join("workspace"),
            )
        })
        .and_then(|_| write_text(&staging.join("winner.txt"), &format!("{}\n", winner.id)))
    {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    if out.exists() {
        std::fs::remove_dir_all(&out).map_err(|error| error.to_string())?;
    }
    if let Err(error) = std::fs::rename(&staging, &out) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_ranking_prefers_passing_small_output() {
        let evidence = vec![
            CastEvidence {
                id: "cast_0".into(),
                gate_ok: false,
                clipped_len: 10,
            },
            CastEvidence {
                id: "cast_1".into(),
                gate_ok: true,
                clipped_len: 100,
            },
            CastEvidence {
                id: "cast_2".into(),
                gate_ok: true,
                clipped_len: 20,
            },
        ];
        assert_eq!(pick_evidence_winner(&evidence, false).expect("winner"), 2);
        assert_eq!(pick_evidence_winner(&evidence, true).expect("winner"), 2);
    }

    #[test]
    fn bradley_terry_pref_is_half_on_equal_strength() {
        assert!((bradley_terry_pref(1.0, 1.0) - 0.5).abs() < 1e-9);
        assert!(bradley_terry_pref(2.0, 0.0) > 0.5);
    }

    #[test]
    fn strict_rejects_when_no_passing_survivor() {
        let evidence = vec![CastEvidence {
            id: "cast_0".into(),
            gate_ok: false,
            clipped_len: 10,
        }];
        let error = pick_evidence_winner(&evidence, true).expect_err("strict");
        assert!(
            error.contains("--strict requires a passing survivor"),
            "{error}"
        );
    }

    #[test]
    fn non_strict_ranks_failing_only_survivor() {
        let evidence = vec![CastEvidence {
            id: "cast_0".into(),
            gate_ok: false,
            clipped_len: 10,
        }];
        assert_eq!(pick_evidence_winner(&evidence, false).expect("winner"), 0);
    }
}
