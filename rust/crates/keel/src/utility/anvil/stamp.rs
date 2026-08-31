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
    let piece = flags.string_value("piece");
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
    let evidence = match load_cast_evidence(paths.as_ref().expect("paths for non-dry run"), piece) {
        Ok(value) => value,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let winners = match select_winners(&evidence, strict) {
        Ok(value) => value,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let value = paths.as_ref().expect("paths for non-dry run");
    if let Err(error) = promote_winners(value, &winners) {
        let _ = writeln!(standard_error, "{error}");
        return 1;
    }
    let line = format!(
        "anvil stamp: winners={} strict={} mode=ppt-evidence",
        winners
            .iter()
            .map(|winner| winner.id.as_str())
            .collect::<Vec<_>>()
            .join(","),
        strict
    );
    let _ = writeln!(standard_output, "{line}");
    let _ = writeln!(
        standard_output,
        "{}",
        supervisor::clip_output(
            &format!(
                "winners {}",
                winners
                    .iter()
                    .map(|winner| format!(
                        "{}:{} gate_ok={} clipped_len={}",
                        winner.piece, winner.id, winner.gate_ok, winner.clipped_len
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
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
    piece: String,
    gate_ok: bool,
    clipped_len: usize,
}

fn load_cast_evidence(
    paths: &job::JobPaths,
    only_piece: &str,
) -> Result<Vec<CastEvidence>, String> {
    let lock = job::load_lock(paths)?;
    let generation = job::generation(&lock)?;
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
            let evidence_generation = value
                .get("generation")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("anvil stamp: {id} evidence lacks generation"))?;
            if evidence_generation != generation {
                return Err(format!(
                    "anvil stamp: {id} evidence belongs to stale generation {evidence_generation}"
                ));
            }
            let piece = value
                .get("piece")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("anvil stamp: {id} evidence lacks piece"))?
                .to_string();
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
                piece,
                gate_ok,
                clipped_len,
            })
        })
        .filter(|result| {
            result
                .as_ref()
                .map(|evidence| only_piece.is_empty() || evidence.piece == only_piece)
                .unwrap_or(true)
        })
        .collect::<Result<Vec<_>, _>>()
        .and_then(|evidence| {
            if evidence.is_empty() {
                Err(if only_piece.is_empty() {
                    "anvil stamp: no cast evidence found".to_string()
                } else {
                    format!("anvil stamp: no cast evidence found for piece {only_piece}")
                })
            } else {
                Ok(evidence)
            }
        })
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

fn select_winners(evidence: &[CastEvidence], strict: bool) -> Result<Vec<CastEvidence>, String> {
    let mut by_piece = std::collections::BTreeMap::<String, Vec<CastEvidence>>::new();
    for candidate in evidence {
        by_piece
            .entry(candidate.piece.clone())
            .or_default()
            .push(candidate.clone());
    }
    let mut winners = Vec::new();
    for candidates in by_piece.into_values() {
        let index = pick_evidence_winner(&candidates, strict)?;
        winners.push(candidates[index].clone());
    }
    if winners.is_empty() {
        return Err("anvil stamp: no cast evidence to rank".to_string());
    }
    Ok(winners)
}

pub(crate) fn ensure_winner_workspace(
    paths: &job::JobPaths,
    strict: bool,
) -> Result<std::path::PathBuf, String> {
    let workspace = paths.out_dir().join("workspace");
    let lock = job::load_lock(paths)?;
    let generation = job::generation(&lock)?;
    if workspace.is_dir()
        && std::fs::read_to_string(paths.out_dir().join("generation.txt"))
            .ok()
            .is_some_and(|value| value.trim() == generation)
    {
        return Ok(workspace);
    }
    let evidence = load_cast_evidence(paths, "").map_err(|error| {
        format!(
            "anvil loop: no promoted winner workspace at {}: {error}",
            workspace.display()
        )
    })?;
    let winners = select_winners(&evidence, strict).map_err(|error| {
        format!(
            "anvil loop: no promoted winner workspace at {}: {error}",
            workspace.display()
        )
    })?;
    let expected_pieces = job::pieces_from_lock(&lock, "")?;
    for piece in &expected_pieces {
        if !winners.iter().any(|winner| winner.piece == piece.id) {
            return Err(format!(
                "anvil loop: no candidate evidence for piece {}",
                piece.id
            ));
        }
    }
    promote_winners(paths, &winners)?;
    if !workspace.is_dir() {
        return Err(format!(
            "anvil loop: no promoted winner workspace at {}",
            workspace.display()
        ));
    }
    Ok(workspace)
}

fn promote_winners(paths: &job::JobPaths, winners: &[CastEvidence]) -> Result<(), String> {
    let out = paths.out_dir();
    let staging = out.with_file_name(format!("anvil_out.tmp-{}", std::process::id()));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|error| error.to_string())?;
    }
    std::fs::create_dir_all(&staging).map_err(|error| error.to_string())?;
    let promoted = (|| {
        let mut results = Vec::new();
        for winner in winners {
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
            let recorded_workspace = value
                .get("workspace")
                .and_then(|item| item.as_str())
                .ok_or_else(|| "anvil stamp: result lacks workspace".to_string())?;
            let workspace = paths.dir.join(&winner.id).join("workspace");
            if std::path::Path::new(recorded_workspace) != workspace {
                return Err(format!(
                    "anvil stamp: {} evidence points outside its owned workspace",
                    winner.id
                ));
            }
            merge_tree(&workspace, &staging.join("workspace"))?;
            results.push(value);
        }
        let lock = job::load_lock(paths)?;
        let generation = job::generation(&lock)?;
        write_text(
            &staging.join("result.json"),
            &serde_json::Value::Array(results).to_string(),
        )?;
        write_text(
            &staging.join("winner.txt"),
            &format!(
                "{}\n",
                winners
                    .iter()
                    .map(|winner| winner.id.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        )?;
        write_text(&staging.join("generation.txt"), &format!("{generation}\n"))?;
        Ok::<(), String>(())
    })();
    if let Err(error) = promoted {
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

fn merge_tree(source: &std::path::Path, destination: &std::path::Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err(format!("anvil merge refuses symlink: {}", source.display()));
    }
    if metadata.is_file() {
        if destination.is_file() {
            let existing = std::fs::read(destination).map_err(|error| error.to_string())?;
            let incoming = std::fs::read(source).map_err(|error| error.to_string())?;
            if existing != incoming {
                return Err(format!(
                    "anvil stamp: candidate pieces conflict at {}",
                    destination.display()
                ));
            }
            return Ok(());
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::copy(source, destination).map_err(|error| error.to_string())?;
        return Ok(());
    }
    if metadata.is_dir() {
        std::fs::create_dir_all(destination).map_err(|error| error.to_string())?;
        for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            merge_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
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
                piece: "main".into(),
                gate_ok: false,
                clipped_len: 10,
            },
            CastEvidence {
                id: "cast_1".into(),
                piece: "main".into(),
                gate_ok: true,
                clipped_len: 100,
            },
            CastEvidence {
                id: "cast_2".into(),
                piece: "main".into(),
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
            piece: "main".into(),
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
            piece: "main".into(),
            gate_ok: false,
            clipped_len: 10,
        }];
        assert_eq!(pick_evidence_winner(&evidence, false).expect("winner"), 0);
    }
}
