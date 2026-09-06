//! ClarifyPacket gate for anvil compile (SUPERHARNESS P1).
//!
//! Artifact path (product ID): `clarify.packet.json` under the workspace anvil
//! bank — `<keel-home>/memories/workspaces/<slug>/anvil/clarify.packet.json`.
//!
//! Answers and AskUser payloads are untrusted: sanitize + size-bound only.
//! Never shell-interpolate, eval, or treat answer text as code.
//! Orchestrator owns host AskUser adapter playbooks; subagents escalate only.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;

use crate::utility::hashing::fnv1a64_hex;

/// Product artifact file name (Designer / PRD contract — do not rename casually).
pub const CLARIFY_PACKET_FILE: &str = "clarify.packet.json";
/// Sentinel that marks the gate required before a packet is written.
pub const CLARIFY_REQUIRED_SENTINEL: &str = "clarify.required";

/// Status token when compile is refused for clarify (UX contract).
pub const STATUS_CLARIFY_BLOCKED: &str = "CLARIFY_BLOCKED";

const MAX_QUESTIONS: usize = 4;
const MIN_QUESTIONS: usize = 1;
const MAX_ANSWER_CHARS: usize = 4_096;
const MAX_TOTAL_ANSWER_CHARS: usize = 16_384;
const MAX_OPTION_CHARS: usize = 256;
const MAX_QUESTION_CHARS: usize = 1_024;
const ALLOWED_DELTA_FIELDS: &[&str] = &["constraints", "acceptance", "non_goals", "open_risks"];

const VALID_TRIGGERS: &[&str] = &[
    "ambiguous_req",
    "multi_path",
    "irreversible_side_effect",
    "missing_env_fact",
    "conflicting_constraints",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClarifyGateError {
    Missing,
    Malformed(String),
    HardBlock {
        missing_ids: Vec<String>,
    },
    Drift(String),
    GoalMismatch {
        locked: String,
        compile_goal: String,
    },
}

impl std::fmt::Display for ClarifyGateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => write!(
                f,
                "{STATUS_CLARIFY_BLOCKED}: missing {CLARIFY_PACKET_FILE} (clarify required)"
            ),
            Self::Malformed(detail) => {
                write!(f, "{STATUS_CLARIFY_BLOCKED}: malformed {CLARIFY_PACKET_FILE}: {detail}")
            }
            Self::HardBlock { missing_ids } => write!(
                f,
                "{STATUS_CLARIFY_BLOCKED}: hard_block — unanswered required questions: {}",
                missing_ids.join(", ")
            ),
            Self::Drift(detail) => write!(f, "{STATUS_CLARIFY_BLOCKED}: drift_check failed: {detail}"),
            Self::GoalMismatch {
                locked,
                compile_goal,
            } => write!(
                f,
                "{STATUS_CLARIFY_BLOCKED}: locked_brief.goal is immutable (locked={locked:?} compile={compile_goal:?})"
            ),
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // schema fields retained for artifact contract
pub struct ClarifyPacket {
    pub version: u64,
    pub trigger: String,
    pub questions: Vec<ClarifyQuestion>,
    pub answers: BTreeMap<String, AnswerValue>,
    pub locked_brief: LockedBrief,
    pub unanswered_policy: String,
    pub drift_check: DriftCheck,
    /// Stored hard_block flag; gate also recomputes from unanswered required Qs.
    pub hard_block: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // schema fields retained for artifact contract
pub struct ClarifyQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    pub qtype: String,
    pub options: Vec<String>,
    pub multi_select: bool,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerValue {
    Text(String),
    List(Vec<String>),
}

impl AnswerValue {
    #[allow(dead_code)] // used by unit tests / future receipts
    pub fn as_display(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::List(items) => items.join(", "),
        }
    }

    pub fn char_len(&self) -> usize {
        match self {
            Self::Text(s) => s.chars().count(),
            Self::List(items) => items.iter().map(|s| s.chars().count()).sum(),
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // schema fields retained for artifact contract
pub struct LockedBrief {
    pub goal: String,
    pub non_goals: Vec<String>,
    pub constraints: Vec<String>,
    pub acceptance: Vec<String>,
    pub open_risks: Vec<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // schema fields retained for artifact contract
pub struct DriftCheck {
    pub original_goal_hash: String,
    pub allowed_delta_fields: Vec<String>,
}

/// Path to the ClarifyPacket artifact for this job bank.
pub fn clarify_packet_path(anvil_dir: &Path) -> PathBuf {
    anvil_dir.join(CLARIFY_PACKET_FILE)
}

/// Path to the optional sentinel that marks clarify required.
pub fn clarify_required_path(anvil_dir: &Path) -> PathBuf {
    anvil_dir.join(CLARIFY_REQUIRED_SENTINEL)
}

/// True when compile must enforce ClarifyPacket (flag, sentinel, or existing packet).
pub fn clarify_gate_required(anvil_dir: &Path, flag_required: bool) -> bool {
    flag_required
        || clarify_required_path(anvil_dir).is_file()
        || clarify_packet_path(anvil_dir).is_file()
}

/// Hash of goal text at gate open (canonical trim). Used by drift_check.
pub fn goal_hash(goal: &str) -> String {
    fnv1a64_hex(goal.trim())
}

/// Sanitize a single untrusted answer string: strip NULs, drop other C0
/// controls except tab/LF/CR, size-bound. Never for shell interpolation.
pub fn sanitize_answer_text(raw: &str) -> Result<String, String> {
    if raw.chars().count() > MAX_ANSWER_CHARS {
        return Err(format!(
            "answer exceeds {MAX_ANSWER_CHARS} characters (untrusted; size-bounded)"
        ));
    }
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch == '\0' {
            continue;
        }
        if ch.is_control() && ch != '\t' && ch != '\n' && ch != '\r' {
            continue;
        }
        out.push(ch);
    }
    Ok(out)
}

/// Validate and parse ClarifyPacket JSON text. Answers are sanitized.
pub fn parse_clarify_packet(text: &str) -> Result<ClarifyPacket, String> {
    let value: JsonValue = serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
    parse_clarify_value(&value)
}

pub fn parse_clarify_value(value: &JsonValue) -> Result<ClarifyPacket, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "top-level must be object".to_string())?;

    let version = obj
        .get("version")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "version required".to_string())?;
    if version != 1 {
        return Err("version must be 1".into());
    }

    let trigger = obj
        .get("trigger")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if trigger.is_empty() || !VALID_TRIGGERS.contains(&trigger.as_str()) {
        return Err(format!(
            "trigger must be one of {}",
            VALID_TRIGGERS.join("|")
        ));
    }

    let questions = parse_questions(obj.get("questions"))?;
    let answers = parse_answers(obj.get("answers"))?;
    let locked_brief = parse_locked_brief(obj.get("locked_brief"))?;
    let unanswered_policy = obj
        .get("unanswered_policy")
        .and_then(|v| v.as_str())
        .unwrap_or("hard_block")
        .trim()
        .to_string();
    if unanswered_policy != "hard_block" {
        return Err("unanswered_policy must be hard_block (founder lock; no AFK continue)".into());
    }
    let drift_check = parse_drift_check(obj.get("drift_check"))?;
    let hard_block = obj
        .get("hard_block")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Ownership is advisory documentation in the artifact; require shape when present.
    if let Some(ownership) = obj.get("ownership") {
        let o = ownership
            .as_object()
            .ok_or_else(|| "ownership must be object".to_string())?;
        let orch = o
            .get("orchestrator")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let subs = o
            .get("subagents")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if orch.is_empty() || subs.is_empty() {
            return Err(
                "ownership.orchestrator and ownership.subagents required when ownership present"
                    .into(),
            );
        }
    }

    Ok(ClarifyPacket {
        version,
        trigger,
        questions,
        answers,
        locked_brief,
        unanswered_policy,
        drift_check,
        hard_block,
    })
}

fn parse_questions(raw: Option<&JsonValue>) -> Result<Vec<ClarifyQuestion>, String> {
    let arr = raw
        .and_then(|v| v.as_array())
        .ok_or_else(|| "questions must be a non-empty array".to_string())?;
    if arr.len() < MIN_QUESTIONS || arr.len() > MAX_QUESTIONS {
        return Err(format!(
            "questions must have {MIN_QUESTIONS}..{MAX_QUESTIONS} items (anti-spam)"
        ));
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(arr.len());
    for (index, item) in arr.iter().enumerate() {
        let obj = item
            .as_object()
            .ok_or_else(|| format!("questions[{index}] must be object"))?;
        let id = obj
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if id.is_empty() {
            return Err(format!("questions[{index}].id required"));
        }
        if !seen.insert(id.clone()) {
            return Err(format!("duplicate question id {id}"));
        }
        let header = obj
            .get("header")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if header.is_empty() {
            return Err(format!("questions[{index}].header required"));
        }
        let question = obj
            .get("question")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if question.is_empty() || question.chars().count() > MAX_QUESTION_CHARS {
            return Err(format!(
                "questions[{index}].question required and ≤{MAX_QUESTION_CHARS} chars"
            ));
        }
        let qtype = obj
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if !matches!(qtype.as_str(), "choice" | "text" | "yesno") {
            return Err(format!("questions[{index}].type must be choice|text|yesno"));
        }
        let mut options = Vec::new();
        if let Some(opts) = obj.get("options").and_then(|v| v.as_array()) {
            for opt in opts {
                let s = opt
                    .as_str()
                    .ok_or_else(|| format!("questions[{index}].options must be strings"))?
                    .trim();
                if s.is_empty() || s.chars().count() > MAX_OPTION_CHARS {
                    return Err(format!(
                        "questions[{index}].options entries must be non-empty ≤{MAX_OPTION_CHARS} chars"
                    ));
                }
                options.push(s.to_string());
            }
        }
        if qtype == "choice" && options.is_empty() {
            return Err(format!("questions[{index}]: choice requires options"));
        }
        if qtype == "yesno" && options.is_empty() {
            options = vec!["yes".into(), "no".into()];
        }
        let multi_select = obj
            .get("multi_select")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let required = obj
            .get("required")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        out.push(ClarifyQuestion {
            id,
            header,
            question,
            qtype,
            options,
            multi_select,
            required,
        });
    }
    Ok(out)
}

fn parse_answers(raw: Option<&JsonValue>) -> Result<BTreeMap<String, AnswerValue>, String> {
    let Some(raw) = raw else {
        return Ok(BTreeMap::new());
    };
    let mut map = BTreeMap::new();
    let mut total_chars = 0usize;
    match raw {
        JsonValue::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                let obj = item
                    .as_object()
                    .ok_or_else(|| format!("answers[{index}] must be object with id+value"))?;
                let id = obj
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if id.is_empty() {
                    return Err(format!("answers[{index}].id required"));
                }
                let value = coerce_answer(obj.get("value").or_else(|| obj.get("answer")), index)?;
                total_chars = total_chars.saturating_add(value.char_len());
                if total_chars > MAX_TOTAL_ANSWER_CHARS {
                    return Err(format!(
                        "answers total exceed {MAX_TOTAL_ANSWER_CHARS} characters"
                    ));
                }
                map.insert(id, value);
            }
        }
        JsonValue::Object(obj) => {
            for (id, value) in obj {
                let id = id.trim();
                if id.is_empty() {
                    return Err("answers map keys must be non-empty ids".into());
                }
                let answer = coerce_answer(Some(value), 0)?;
                total_chars = total_chars.saturating_add(answer.char_len());
                if total_chars > MAX_TOTAL_ANSWER_CHARS {
                    return Err(format!(
                        "answers total exceed {MAX_TOTAL_ANSWER_CHARS} characters"
                    ));
                }
                map.insert(id.to_string(), answer);
            }
        }
        _ => return Err("answers must be array or object map".into()),
    }
    Ok(map)
}

fn coerce_answer(raw: Option<&JsonValue>, index: usize) -> Result<AnswerValue, String> {
    let Some(raw) = raw else {
        return Err(format!("answers[{index}].value required"));
    };
    match raw {
        JsonValue::String(s) => {
            let cleaned = sanitize_answer_text(s)?;
            Ok(AnswerValue::Text(cleaned))
        }
        JsonValue::Array(items) => {
            let mut list = Vec::with_capacity(items.len());
            for item in items {
                let s = item
                    .as_str()
                    .ok_or_else(|| format!("answers[{index}] list values must be strings"))?;
                list.push(sanitize_answer_text(s)?);
            }
            Ok(AnswerValue::List(list))
        }
        JsonValue::Bool(b) => Ok(AnswerValue::Text(if *b {
            "yes".into()
        } else {
            "no".into()
        })),
        JsonValue::Number(n) => {
            let s = n.to_string();
            Ok(AnswerValue::Text(sanitize_answer_text(&s)?))
        }
        JsonValue::Null => Ok(AnswerValue::Text(String::new())),
        _ => Err(format!(
            "answers[{index}].value must be string|string[]|bool|number"
        )),
    }
}

fn parse_locked_brief(raw: Option<&JsonValue>) -> Result<LockedBrief, String> {
    let obj = raw
        .and_then(|v| v.as_object())
        .ok_or_else(|| "locked_brief required".to_string())?;
    let goal = obj
        .get("goal")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if goal.is_empty() {
        return Err("locked_brief.goal required".into());
    }
    Ok(LockedBrief {
        goal,
        non_goals: string_list(obj.get("non_goals"), "locked_brief.non_goals")?,
        constraints: string_list(obj.get("constraints"), "locked_brief.constraints")?,
        acceptance: string_list(obj.get("acceptance"), "locked_brief.acceptance")?,
        open_risks: string_list(obj.get("open_risks"), "locked_brief.open_risks")?,
    })
}

fn string_list(raw: Option<&JsonValue>, label: &str) -> Result<Vec<String>, String> {
    match raw {
        None => Ok(Vec::new()),
        Some(JsonValue::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let s = item
                    .as_str()
                    .ok_or_else(|| format!("{label} entries must be strings"))?
                    .trim();
                if !s.is_empty() {
                    out.push(s.to_string());
                }
            }
            Ok(out)
        }
        Some(JsonValue::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                Ok(Vec::new())
            } else {
                Ok(vec![t.to_string()])
            }
        }
        _ => Err(format!("{label} must be array or string")),
    }
}

fn parse_drift_check(raw: Option<&JsonValue>) -> Result<DriftCheck, String> {
    let obj = raw
        .and_then(|v| v.as_object())
        .ok_or_else(|| "drift_check required".to_string())?;
    let original_goal_hash = obj
        .get("original_goal_hash")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if original_goal_hash.is_empty() {
        return Err("drift_check.original_goal_hash required".into());
    }
    let allowed = match obj.get("allowed_delta_fields") {
        None => ALLOWED_DELTA_FIELDS
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
        Some(JsonValue::Array(items)) => {
            let mut fields = Vec::new();
            for item in items {
                let s = item
                    .as_str()
                    .ok_or_else(|| "allowed_delta_fields must be strings".to_string())?
                    .trim();
                if !ALLOWED_DELTA_FIELDS.contains(&s) {
                    return Err(format!(
                        "allowed_delta_fields may only include {}",
                        ALLOWED_DELTA_FIELDS.join("|")
                    ));
                }
                fields.push(s.to_string());
            }
            if fields.is_empty() {
                return Err("allowed_delta_fields must not be empty".into());
            }
            fields
        }
        _ => return Err("allowed_delta_fields must be array".into()),
    };
    Ok(DriftCheck {
        original_goal_hash,
        allowed_delta_fields: allowed,
    })
}

/// Required question ids that still lack a non-empty answer.
pub fn unanswered_required(packet: &ClarifyPacket) -> Vec<String> {
    let mut missing = Vec::new();
    for q in &packet.questions {
        if !q.required {
            continue;
        }
        match packet.answers.get(&q.id) {
            None => missing.push(q.id.clone()),
            Some(AnswerValue::Text(s)) if s.trim().is_empty() => missing.push(q.id.clone()),
            Some(AnswerValue::List(items)) if items.iter().all(|s| s.trim().is_empty()) => {
                missing.push(q.id.clone())
            }
            Some(_) => {}
        }
    }
    missing
}

/// Fail-closed: unanswered required ⇒ hard_block regardless of stored flag.
pub fn is_hard_blocked(packet: &ClarifyPacket) -> bool {
    packet.hard_block || !unanswered_required(packet).is_empty()
}

/// Drift: locked goal hash must match original_goal_hash; goal is not an allowed delta.
pub fn check_drift(packet: &ClarifyPacket) -> Result<(), String> {
    let current = goal_hash(&packet.locked_brief.goal);
    if current != packet.drift_check.original_goal_hash {
        return Err(format!(
            "locked_brief.goal hash {current} != original_goal_hash {} (goal is immutable; open a new ClarifyPacket)",
            packet.drift_check.original_goal_hash
        ));
    }
    if packet
        .drift_check
        .allowed_delta_fields
        .iter()
        .any(|f| f == "goal")
    {
        return Err("allowed_delta_fields must not include goal".into());
    }
    Ok(())
}

/// Load + validate packet from disk when the gate is required.
pub fn enforce_clarify_for_compile(
    anvil_dir: &Path,
    compile_goal: &str,
    flag_required: bool,
) -> Result<Option<ClarifyPacket>, ClarifyGateError> {
    if !clarify_gate_required(anvil_dir, flag_required) {
        return Ok(None);
    }
    let path = clarify_packet_path(anvil_dir);
    if !path.is_file() {
        return Err(ClarifyGateError::Missing);
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| ClarifyGateError::Malformed(format!("read {}: {e}", path.display())))?;
    let packet = parse_clarify_packet(&text).map_err(ClarifyGateError::Malformed)?;
    if is_hard_blocked(&packet) {
        return Err(ClarifyGateError::HardBlock {
            missing_ids: unanswered_required(&packet),
        });
    }
    check_drift(&packet).map_err(ClarifyGateError::Drift)?;
    let locked = packet.locked_brief.goal.trim();
    let goal = compile_goal.trim();
    if locked != goal {
        return Err(ClarifyGateError::GoalMismatch {
            locked: locked.to_string(),
            compile_goal: goal.to_string(),
        });
    }
    Ok(Some(packet))
}

/// AskUser adapter playbook notes (orchestrator-owned; not executed by this crate).
pub fn ask_user_adapter_playbook() -> &'static str {
    "ClarifyPacket AskUser adapters (orchestrator owns; subagents escalate only):\n\
     - Claude Code: AskUserQuestion / equivalent; write answers into clarify.packet.json; never eval.\n\
     - Cursor: host ask_user / pause; same artifact semantics.\n\
     - Gemini / Antigravity: AskQuestion or external pause; same artifact.\n\
     - Host with no ask-user: external pause; status CLARIFY_BLOCKED; no AFK continue.\n\
     Answers + AskUser payloads are untrusted data — sanitize/size-bound only; never shell-interpolate."
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_packet_json(goal: &str, answer: &str, hard_block: bool) -> String {
        let hash = goal_hash(goal);
        serde_json::to_string_pretty(&json!({
            "version": 1,
            "artifact_id": "clarify.packet.json",
            "trigger": "ambiguous_req",
            "questions": [{
                "id": "scope",
                "header": "Scope",
                "question": "Which surface?",
                "type": "choice",
                "options": ["cli", "docs"],
                "required": true
            }],
            "answers": [{"id": "scope", "value": answer}],
            "locked_brief": {
                "goal": goal,
                "non_goals": ["P2 design artifacts"],
                "constraints": ["MIT only"],
                "acceptance": ["compile refuses when gated without packet"],
                "open_risks": ["host ask-user uneven"]
            },
            "unanswered_policy": "hard_block",
            "drift_check": {
                "original_goal_hash": hash,
                "allowed_delta_fields": ["constraints", "acceptance", "non_goals", "open_risks"]
            },
            "hard_block": hard_block,
            "ownership": {
                "orchestrator": "owns AskUser adapter mapping",
                "subagents": "MUST escalate to main — must not answer or skip"
            }
        }))
        .unwrap()
    }

    #[test]
    fn parses_valid_packet_and_sanitizes_answers() {
        let text = valid_packet_json("ship clarify gate", "cli\0\u{0007}ok", false);
        let packet = parse_clarify_packet(&text).expect("parse");
        assert_eq!(packet.answers.get("scope").unwrap().as_display(), "cliok");
        assert!(!is_hard_blocked(&packet));
        check_drift(&packet).expect("drift");
    }

    #[test]
    fn unanswered_required_is_hard_block() {
        let text = valid_packet_json("ship clarify gate", "", false);
        let packet = parse_clarify_packet(&text).expect("parse");
        assert!(is_hard_blocked(&packet));
        assert_eq!(unanswered_required(&packet), vec!["scope".to_string()]);
    }

    #[test]
    fn drift_rejects_goal_rewrite() {
        let mut text = valid_packet_json("original goal", "cli", false);
        // Tamper locked goal while keeping old hash.
        let mut value: JsonValue = serde_json::from_str(&text).unwrap();
        value["locked_brief"]["goal"] = json!("injected new product goal");
        text = serde_json::to_string(&value).unwrap();
        let packet = parse_clarify_packet(&text).expect("parse");
        assert!(check_drift(&packet).is_err());
    }

    #[test]
    fn immutable_goal_must_match_compile_goal() {
        let dir = std::env::temp_dir().join(format!(
            "clarify-goal-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = clarify_packet_path(&dir);
        std::fs::write(&path, valid_packet_json("locked goal", "cli", false)).unwrap();
        let err = enforce_clarify_for_compile(&dir, "different goal", true).expect_err("mismatch");
        assert!(matches!(err, ClarifyGateError::GoalMismatch { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_packet_refuses_when_gated() {
        let dir = std::env::temp_dir().join(format!(
            "clarify-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let err = enforce_clarify_for_compile(&dir, "any", true).expect_err("missing");
        assert!(matches!(err, ClarifyGateError::Missing));
        assert!(err.to_string().contains(STATUS_CLARIFY_BLOCKED));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_packet_refuses_when_gated() {
        let dir = std::env::temp_dir().join(format!(
            "clarify-bad-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(clarify_packet_path(&dir), "{not-json").unwrap();
        let err = enforce_clarify_for_compile(&dir, "g", false).expect_err("malformed");
        assert!(matches!(err, ClarifyGateError::Malformed(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_afk_continue_policy() {
        let hash = goal_hash("g");
        let value = json!({
            "version": 1,
            "trigger": "ambiguous_req",
            "questions": [{
                "id": "q1",
                "header": "H",
                "question": "Q?",
                "type": "yesno",
                "required": true
            }],
            "answers": [],
            "locked_brief": {"goal": "g", "non_goals": [], "constraints": [], "acceptance": [], "open_risks": []},
            "unanswered_policy": "afk_continue",
            "drift_check": {"original_goal_hash": hash, "allowed_delta_fields": ["constraints"]},
            "hard_block": false
        });
        assert!(parse_clarify_value(&value).is_err());
    }

    #[test]
    fn ungated_compile_skips_packet() {
        let dir = std::env::temp_dir().join(format!(
            "clarify-skip-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let result = enforce_clarify_for_compile(&dir, "clear goal", false).expect("ok");
        assert!(result.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn playbook_names_hosts_and_untrusted_rule() {
        let text = ask_user_adapter_playbook();
        assert!(text.contains("Claude"));
        assert!(text.contains("Cursor"));
        assert!(text.contains("Gemini"));
        assert!(text.contains("untrusted"));
        assert!(text.contains("shell-interpolate"));
    }
}
