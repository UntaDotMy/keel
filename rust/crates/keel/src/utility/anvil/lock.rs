use std::collections::HashSet;

use serde_json::Value as JsonValue;

const CATEGORY_WORDS: &[&str] = &[
    "good ux",
    "production quality",
    "award-winning",
    "high quality",
    "best in class",
];

pub fn validate_lock(text: &str) -> Result<JsonValue, String> {
    let value: JsonValue =
        serde_json::from_str(text).map_err(|e| format!("lock: invalid JSON: {e}"))?;
    validate_value(&value)?;
    let canonical = canonical_json(&value);
    let reparsed: JsonValue =
        serde_json::from_str(&canonical).map_err(|e| format!("lock: canonical round-trip: {e}"))?;
    if reparsed != value {
        return Err("lock: JSON must round-trip with sorted keys".into());
    }
    Ok(value)
}

fn validate_value(v: &JsonValue) -> Result<(), String> {
    let obj = v.as_object().ok_or("lock: top-level must be object")?;
    let version = obj
        .get("version")
        .and_then(|x| x.as_u64())
        .ok_or("lock: version required")?;
    if version != 1 {
        return Err("lock: version must be 1".into());
    }
    let goal = obj
        .get("goal")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim();
    if goal.is_empty() {
        return Err("lock: goal required".into());
    }
    let bar = obj
        .get("bar")
        .and_then(|x| x.as_object())
        .ok_or("lock: bar required")?;
    let name = bar
        .get("name")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim();
    if name.is_empty() {
        return Err("lock: bar.name required".into());
    }
    let lower = name.to_ascii_lowercase();
    for w in CATEGORY_WORDS {
        if lower == *w {
            return Err(format!("lock: bar.name must be named, not category {w:?}"));
        }
    }
    let fetch = bar
        .get("fetch")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim();
    if !(fetch.starts_with("cmd:")
        || fetch.starts_with("url:")
        || fetch.starts_with("file:")
        || fetch.starts_with("git:"))
    {
        return Err("lock: bar.fetch must start with cmd:, url:, file:, or git:".into());
    }
    let budget = obj
        .get("budget")
        .and_then(|x| x.as_object())
        .ok_or("lock: budget required")?;
    let n_casts = budget.get("n_casts").and_then(|x| x.as_u64()).unwrap_or(3);
    if !(1..=8).contains(&n_casts) {
        return Err("lock: budget.n_casts must be 1..8".into());
    }
    let k_pivots = budget.get("k_pivots").and_then(|x| x.as_u64()).unwrap_or(1);
    if k_pivots >= n_casts {
        return Err("lock: budget.k_pivots must be < n_casts".into());
    }
    if let Some(v) = budget.get("max_iterations").and_then(|x| x.as_u64()) {
        if !(5..=50).contains(&v) {
            return Err("lock: budget.max_iterations must be 5..50".into());
        }
    }
    if let Some(v) = budget
        .get("min_improvement_threshold")
        .and_then(|x| x.as_f64())
    {
        if !(0.0..=1.0).contains(&v) {
            return Err("lock: budget.min_improvement_threshold must be 0..1".into());
        }
    }
    let models = obj
        .get("models")
        .and_then(|x| x.as_object())
        .ok_or("lock: models required")?;
    let allow = models
        .get("allow_training_data")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    if !allow {
        for key in ["compile", "cast", "stamp", "loop"] {
            if let Some(id) = models.get(key).and_then(|x| x.as_str()) {
                let low = id.to_ascii_lowercase();
                if low.contains("contributor") || low.contains("train") || low.contains("free-data")
                {
                    return Err(format!(
                        "lock: models.{key} {id:?} forbidden when allow_training_data=false"
                    ));
                }
            }
        }
    }
    let criteria = obj
        .get("criteria")
        .and_then(|x| x.as_array())
        .ok_or("lock: criteria required")?;
    let expected = ["specification", "output", "errors"];
    if criteria.len() != 3 {
        return Err("lock: criteria must be [specification, output, errors]".into());
    }
    for (i, exp) in expected.iter().enumerate() {
        if criteria[i].as_str() != Some(*exp) {
            return Err("lock: criteria must be [specification, output, errors]".into());
        }
    }
    let pieces = obj
        .get("pieces")
        .and_then(|x| x.as_array())
        .ok_or("lock: pieces required")?;
    if pieces.is_empty() {
        return Err("lock: pieces must be non-empty".into());
    }
    let mut ids = HashSet::new();
    let has_blind = pieces
        .iter()
        .any(|p| p.get("critic").and_then(|x| x.as_str()) == Some("blind_ab"));
    if has_blind && n_casts < 2 {
        return Err("lock: n_casts must be >=2 when any blind_ab".into());
    }
    for p in pieces {
        let o = p.as_object().ok_or("lock: piece must be object")?;
        let id = o.get("id").and_then(|x| x.as_str()).unwrap_or("").trim();
        if id.is_empty() {
            return Err("lock: piece.id required".into());
        }
        if !ids.insert(id.to_string()) {
            return Err(format!("lock: duplicate piece id {id:?}"));
        }
        let critic = o.get("critic").and_then(|x| x.as_str()).unwrap_or("");
        if !matches!(critic, "none" | "blind_ab") {
            return Err(format!("lock: piece {id} critic must be none|blind_ab"));
        }
        let gates = o.get("gates").and_then(|x| x.as_array());
        if critic == "none" && gates.map_or(true, |g| g.is_empty()) {
            return Err(format!(
                "lock: piece {id} critic:none requires at least one gate"
            ));
        }
    }
    Ok(())
}

pub fn canonical_json(value: &JsonValue) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_lock(allow: bool, n_casts: u64) -> String {
        serde_json::json!({
            "version": 1,
            "goal": "CLI that pretty-prints JSON logs",
            "bar": {"name": "jq 1.7", "fetch": "cmd:jq --version", "compare": "stdout+exit"},
            "budget": {"n_casts": n_casts, "k_pivots": 1, "critic_k": 1, "granularity": 20, "builder_retries": 2, "max_tokens_cast": 80000, "max_tokens_stamp": 40000, "max_tokens_loop": 100000, "max_tool_chars": 4000, "max_iterations": 20, "min_improvement_threshold": 0.05},
            "models": {"compile": "frontier", "cast": "cheap", "stamp": "mid", "loop": "cheap", "allow_training_data": allow},
            "criteria": ["specification","output","errors"],
            "pieces": [{"id":"parse","files":["src/parse.py"],"gates":["pytest -q tests/test_parse.py"],"critic":"none"}]
        }).to_string()
    }

    #[test]
    fn rejects_unnamed_bar() {
        let mut v: JsonValue = serde_json::from_str(&minimal_lock(true, 3)).unwrap();
        v["bar"]["name"] = JsonValue::String("good UX".into());
        assert!(validate_value(&v).is_err());
    }

    #[test]
    fn rejects_contributor_when_disallowed() {
        let mut v: JsonValue = serde_json::from_str(&minimal_lock(false, 3)).unwrap();
        v["models"]["cast"] = JsonValue::String("contributor-free".into());
        assert!(validate_value(&v).is_err());
    }

    #[test]
    fn rejects_n_casts_vs_blind() {
        let text = minimal_lock(false, 1).replace("\"critic\":\"none\"", "\"critic\":\"blind_ab\"");
        let v: JsonValue = serde_json::from_str(&text).unwrap();
        assert!(validate_value(&v).is_err());
    }

    #[test]
    fn rejects_max_iterations_bounds() {
        let mut v: JsonValue = serde_json::from_str(&minimal_lock(true, 3)).unwrap();
        v["budget"]["max_iterations"] = JsonValue::Number(serde_json::Number::from(100));
        assert!(validate_value(&v).is_err());
    }
}
