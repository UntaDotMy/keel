use std::collections::HashMap;
use std::io::Write;

use crate::args::FlagSet;
use crate::utility::anvil::job;
use crate::utility::anvil::supervisor;

pub fn ev_score(logprobs: &[(char, f64)], phi: &dyn Fn(char) -> f64) -> f64 {
    logprobs
        .iter()
        .map(|(letter, prob)| prob * phi(*letter))
        .sum()
}

pub fn bradley_terry(r_a: f64, r_b: f64) -> f64 {
    1.0 / (1.0 + (-(r_a - r_b)).exp())
}

pub struct PptResult {
    pub winner: usize,
    #[allow(dead_code)]
    pub comparisons: usize,
}

pub fn ppt_pick(n: usize, k: usize, p_matrix: &dyn Fn(usize, usize) -> f64) -> PptResult {
    if n == 0 {
        return PptResult {
            winner: 0,
            comparisons: 0,
        };
    }
    let mut wins = vec![0.0; n];
    let mut counts = vec![0usize; n];
    let mut comps = HashMap::new();
    let ring: Vec<usize> = (0..n).collect();
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        let key = (a.min(b), a.max(b));
        let p = *comps.entry(key).or_insert_with(|| p_matrix(a, b));
        let p_ab = if a < b { p } else { 1.0 - p };
        wins[a] += p_ab;
        wins[b] += 1.0 - p_ab;
        counts[a] += 1;
        counts[b] += 1;
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|a, b| {
        let sa = if counts[*a] > 0 {
            wins[*a] / counts[*a] as f64
        } else {
            0.0
        };
        let sb = if counts[*b] > 0 {
            wins[*b] / counts[*b] as f64
        } else {
            0.0
        };
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });
    let pivots: Vec<usize> = order[..k.min(n)].to_vec();
    let non_pivots: Vec<usize> = (0..n).filter(|x| !pivots.contains(x)).collect();
    for np in &non_pivots {
        for pv in &pivots {
            let a = *np;
            let b = *pv;
            let key = (a.min(b), a.max(b));
            if comps.contains_key(&key) {
                continue;
            }
            let p = p_matrix(a, b);
            let p_ab = if a < b { p } else { 1.0 - p };
            wins[a] += p_ab;
            wins[b] += 1.0 - p_ab;
            counts[a] += 1;
            counts[b] += 1;
            comps.insert(key, p);
        }
    }
    let mut best = 0;
    let mut best_score: f64 = -1.0;
    for i in 0..n {
        let score = if counts[i] > 0 {
            wins[i] / counts[i] as f64
        } else {
            0.0
        };
        if score > best_score {
            best_score = score;
            best = i;
        }
    }
    PptResult {
        winner: best,
        comparisons: comps.len(),
    }
}

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
    if !dry_run && casts.len() < 2 {
        if casts.len() == 1 {
            if let Some(value) = paths.as_ref() {
                if let Err(error) = promote_winner(value, 0) {
                    let _ = writeln!(standard_error, "{error}");
                    return 1;
                }
            }
            let _ = writeln!(
                standard_output,
                "anvil stamp: winner=0 K=0 mode=single (skip PPT, <2 survivors)"
            );
            return 0;
        }
        let _ = writeln!(standard_error, "anvil stamp: no survivors to score");
        return 1;
    }
    let n = if casts.len() >= 2 {
        casts.len()
    } else if strict {
        3
    } else {
        2
    };
    let k = if strict { 2usize } else { 1usize };
    let strengths: Vec<f64> = (0..n).map(|i| 0.9 - (i as f64) * 0.2).collect();
    let phi = |letter: char| (letter as u8 - b'A' + 1) as f64;
    let high = ev_score(&[('T', 0.7), ('A', 0.3)], &phi);
    let low = ev_score(&[('A', 0.7), ('T', 0.3)], &phi);
    let pair = bradley_terry(high, low);
    let picked = ppt_pick(n, k, &|a, b| {
        if strengths[a] > strengths[b] {
            pair
        } else {
            1.0 - pair
        }
    });
    if let Some(value) = paths.as_ref() {
        if !dry_run && !casts.is_empty() {
            if let Err(error) = promote_winner(value, picked.winner) {
                let _ = writeln!(standard_error, "{error}");
                return 1;
            }
        }
    }
    let mode = "ppt";
    let line = format!(
        "anvil stamp: EV high={high:.2} low={low:.2} p={pair:.2} winner={} K={k} mode={mode}",
        picked.winner
    );
    let _ = writeln!(standard_output, "{line}");
    let _ = writeln!(
        standard_output,
        "{}",
        supervisor::clip_output(&format!("winner {}", picked.winner), 4000)
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

fn promote_winner(paths: &job::JobPaths, winner: usize) -> Result<(), String> {
    let out = paths.out_dir();
    std::fs::create_dir_all(&out).map_err(|error| error.to_string())?;
    let result = paths.dir.join(format!("cast_{winner}")).join("result.json");
    if result.is_file() {
        std::fs::copy(&result, out.join("result.json")).map_err(|error| error.to_string())?;
        if let Ok(text) = std::fs::read_to_string(&result) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(workspace) = value.get("workspace").and_then(|item| item.as_str()) {
                    crate::utility::anvil::workspace::copy_tree(
                        std::path::Path::new(workspace),
                        &out.join("workspace"),
                    )?;
                }
            }
        }
    }
    std::fs::write(out.join("winner.txt"), format!("cast_{winner}\n"))
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ev_ranks_correctly() {
        let phi = |letter: char| (letter as u8 - b'A' + 1) as f64;
        let high = vec![('T', 0.9), ('A', 0.1)];
        let low = vec![('A', 0.9), ('T', 0.1)];
        assert!(ev_score(&high, &phi) > ev_score(&low, &phi));
    }

    #[test]
    fn ppt_picks_argmax() {
        let strengths = [0.9, 0.5, 0.2];
        let result = ppt_pick(3, 1, &|a, b| {
            if strengths[a] > strengths[b] {
                0.8
            } else {
                0.2
            }
        });
        assert_eq!(result.winner, 0);
        assert!(result.comparisons <= 5);
    }

    #[test]
    fn ppt_pair_cache_no_dup() {
        let calls = std::cell::Cell::new(0);
        let _ = ppt_pick(3, 1, &|_, _| {
            calls.set(calls.get() + 1);
            0.5
        });
        assert!(calls.get() <= 5);
    }
}
