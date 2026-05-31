//! Purpose: Deterministic prompt→skill matcher. Reads the installed
//!   `~/.claude/skills/<name>/SKILL.md` frontmatter, builds an IDF-weighted
//!   keyword model per skill, and decides whether a user prompt is a *strong,
//!   distinctive* match for exactly one skill.
//! Caller: `runner::hook_lifecycle::run_hook_user_prompt_submit` — on a strong,
//!   distinctive match it injects a bounded slice of the matched skill's *actual
//!   body* into the per-prompt context (see `skill_inline_brief`), so the
//!   operative guidance lands whether or not the gateway model chooses to honor
//!   a `Skill("<name>")` tool call. The match itself is the gate; the inline
//!   brief is what makes it model-independent.
//! Dependencies: std::fs, std::path, std::collections; crate::runtime for the
//!   skills directory resolver.
//! Main Functions: match_skill_for_prompt, load_skill_terms,
//!   score_prompt_against_skills, skill_inline_brief.
//! Side Effects: Reads SKILL.md files under the installed skills directory.
//!   Never writes. Any IO failure degrades to "no match" so the hook fails
//!   open to its generic reminder.
//!
//! Why deterministic, not model-driven: this runs inside a UserPromptSubmit
//! hook on every turn. It must be fast, dependency-free, and incapable of
//! mis-routing a generic prompt. The matcher is intentionally conservative —
//! it stays silent unless one skill both clears an absolute score floor and
//! beats the runner-up by a margin on a *distinctive* (corpus-rare) token. A
//! silent matcher is correct; a confidently-wrong one is worse than the
//! existing generic nudge.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use crate::runtime::skills_directory;

/// Score floor as a fraction of `ln(corpus_size)`. The floor must scale with
/// corpus size because IDF does: a token present in exactly one skill scores
/// `ln(N)`, which is ~3.56 for the real ~35-skill install but only ~1.95 for a
/// small test corpus. An *absolute* floor calibrated for one corpus size
/// silently mis-fires on another. Expressing it as a fraction of `ln(N)` keeps
/// the bar at "roughly one distinctive token" regardless of how many skills are
/// installed. At 0.75: a unique (df=1) or near-unique (df=2) token clears,
/// while a borderline df=3 token needs a name-boost or a second hit to qualify.
const MIN_SCORE_FACTOR: f64 = 0.75;

/// The winner must beat the runner-up by this factor. Prevents firing when two
/// skills are near-ties (ambiguous prompt) — exactly the case where naming one
/// would be a coin-flip mis-route.
const DISTINCTIVENESS_MARGIN: f64 = 1.25;

/// A token is "distinctive" when it appears in at most this many skills. The
/// winning overlap must include at least one distinctive token, so a pile of
/// generic words can never trigger a match on its own.
const DISTINCTIVE_DF_MAX: usize = 3;

/// Skill-name tokens are the canonical handle for a skill, so a prompt that
/// uses them is a stronger signal than an incidental description-word hit.
const NAME_TOKEN_BOOST: f64 = 1.5;

/// Hard cap on the inline brief injected into per-prompt context. Skill bodies
/// run ~10-15 KB; we inject the description plus the opening body and stop at
/// this many bytes (rounded out to the next line boundary). The cap keeps the
/// per-prompt input-token cost bounded — the full skill is still one
/// `Skill("<name>")` call away for the model that wants the rest — while
/// guaranteeing the operative guidance lands even if the gateway model never
/// makes that call. ~2400 bytes is roughly 600 tokens: enough for a skill's
/// purpose and its first one or two discipline sections, small enough to pay
/// every prompt that distinctively matches.
const INLINE_BRIEF_MAX_BYTES: usize = 2400;

/// Tokenized term model for one installed skill.
#[derive(Debug, Clone)]
pub struct SkillTerms {
    pub name: String,
    /// Every token drawn from name + description + when_to_use.
    pub all_tokens: HashSet<String>,
    /// Tokens drawn from the skill *name* only (e.g. `stripe`, `integration`).
    pub name_tokens: HashSet<String>,
}

/// A confident, distinctive match.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillMatch {
    pub name: String,
    pub score: f64,
}

/// Resolve the installed skills directory for `claude_home`, load every skill's
/// term model, and return the single distinctive match for `prompt` — or `None`
/// when no skill clears the bar. `None` is the common, correct case for
/// generic prompts; the caller falls back to its generic reminder.
pub fn match_skill_for_prompt(claude_home: &Path, prompt: &str) -> Option<SkillMatch> {
    if prompt.trim().is_empty() {
        return None;
    }
    let skills = load_skill_terms(&skills_directory(claude_home));
    if skills.is_empty() {
        return None;
    }
    score_prompt_against_skills(prompt, &skills)
}

/// Read the matched skill's `SKILL.md` and return a bounded, ready-to-inject
/// brief: the frontmatter `description` followed by the opening of the skill
/// body, truncated to [`INLINE_BRIEF_MAX_BYTES`] on a line boundary. Returns
/// `None` when the skill cannot be read or has no usable content.
///
/// This is the model-independence lever. The per-prompt hook used to *ask* the
/// model to call `Skill("<name>")`; whether that happened depended on the
/// gateway model honoring an injected instruction. Injecting the brief instead
/// means the skill's operative guidance is in the model's input context for
/// this turn no matter what — the `Skill()` call becomes an optional upgrade to
/// the full body, not a prerequisite for any guidance at all.
pub fn skill_inline_brief(claude_home: &Path, skill_name: &str) -> Option<String> {
    // `skill_name` comes from a matched installed skill (the matcher only
    // returns names it read from a real directory), but guard the path join
    // against separators defensively so a crafted frontmatter `name` can never
    // escape the skills directory.
    if skill_name.is_empty()
        || skill_name.contains(['/', '\\'])
        || skill_name.contains("..")
        || Path::new(skill_name).is_absolute()
    {
        return None;
    }
    let skill_path = skills_directory(claude_home)
        .join(skill_name)
        .join("SKILL.md");
    let text = fs::read_to_string(&skill_path).ok()?;
    inline_brief_from_source(&text)
}

/// Pure brief builder (no IO) so truncation behavior is unit-testable. Takes raw
/// SKILL.md text, pulls the frontmatter `description`, drops the frontmatter
/// block, and appends the opening body up to the byte cap on a line boundary.
fn inline_brief_from_source(text: &str) -> Option<String> {
    let body = strip_frontmatter_block(text);
    let description = split_frontmatter(text)
        .and_then(|frontmatter| frontmatter_field(&frontmatter, "description"))
        .unwrap_or_default();

    let mut brief = String::new();
    if !description.trim().is_empty() {
        brief.push_str(description.trim());
        brief.push_str("\n\n");
    }
    brief.push_str(truncate_on_line_boundary(body.trim_start(), INLINE_BRIEF_MAX_BYTES).trim_end());

    let trimmed = brief.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Return everything after the leading `---\n...\n---\n` frontmatter block, or
/// the whole text when there is no frontmatter. Mirrors [`split_frontmatter`]
/// but yields the body rather than the fenced metadata.
fn strip_frontmatter_block(text: &str) -> &str {
    let trimmed_start = text.trim_start_matches(['\u{feff}', ' ', '\t', '\n', '\r']);
    if !trimmed_start.starts_with("---") {
        return text;
    }
    // Skip the opening fence line, then find the closing fence.
    let Some(after_open) = trimmed_start.split_once('\n').map(|(_, rest)| rest) else {
        return text;
    };
    let mut offset = 0usize;
    for line in after_open.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']).trim() == "---" {
            return &after_open[offset + line.len()..];
        }
        offset += line.len();
    }
    // Unterminated frontmatter — no usable body.
    ""
}

/// Truncate `text` to at most `max_bytes`, backing up to the last newline so the
/// brief never ends mid-line. Falls back to a hard byte cut on a char boundary
/// when the first line already exceeds the cap. Appends an explicit elision
/// marker when content was dropped so the model knows the skill continues.
fn truncate_on_line_boundary(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    // Prefer cutting at the last newline within the budget.
    let window = &text.as_bytes()[..max_bytes];
    let cut = window
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|pos| pos + 1)
        .unwrap_or_else(|| {
            // No newline in range — back up to a UTF-8 char boundary.
            let mut end = max_bytes;
            while end > 0 && !text.is_char_boundary(end) {
                end -= 1;
            }
            end
        });
    let mut truncated = text[..cut].trim_end().to_string();
    truncated.push_str("\n\n[skill brief truncated — call Skill(\"<name>\") for the full body]");
    truncated
}

/// Read `<skills_dir>/<name>/SKILL.md` for every installed skill and build its
/// term model. Skips `_shared` and any directory without a SKILL.md. Any read
/// or parse failure for one skill drops that skill silently rather than failing
/// the whole match.
pub fn load_skill_terms(skills_dir: &Path) -> Vec<SkillTerms> {
    let entries = match fs::read_dir(skills_dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut models = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        // `_shared` holds cross-skill resources, not a skill. Hidden dirs too.
        if dir_name.starts_with('.') || dir_name.starts_with('_') {
            continue;
        }
        let skill_path = entry.path().join("SKILL.md");
        let text = match fs::read_to_string(&skill_path) {
            Ok(text) => text,
            Err(_) => continue,
        };
        if let Some(model) = skill_terms_from_source(&dir_name, &text) {
            models.push(model);
        }
    }
    models
}

/// Build a [`SkillTerms`] from a directory name and raw SKILL.md text.
/// Uses the frontmatter `description` + `when_to_use` plus the directory name
/// as the matchable surface — exactly the fields the Claude Code matcher reads.
fn skill_terms_from_source(dir_name: &str, text: &str) -> Option<SkillTerms> {
    let frontmatter = split_frontmatter(text)?;
    let description = frontmatter_field(&frontmatter, "description").unwrap_or_default();
    let when_to_use = frontmatter_field(&frontmatter, "when_to_use").unwrap_or_default();
    // Prefer the frontmatter `name`, fall back to the directory name.
    let name = frontmatter_field(&frontmatter, "name")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| dir_name.to_string());

    let name_tokens = tokenize(&name.replace('-', " "));
    let mut all_tokens = name_tokens.clone();
    all_tokens.extend(tokenize(&description));
    all_tokens.extend(tokenize(&when_to_use));

    if all_tokens.is_empty() {
        return None;
    }
    Some(SkillTerms {
        name,
        all_tokens,
        name_tokens,
    })
}

/// Pure scoring core (no IO) so the thresholds are unit-testable against
/// synthetic corpora. Computes per-token IDF across the supplied skills, scores
/// each skill by the IDF-weighted overlap with the prompt tokens, and returns
/// the winner only when it clears [`MIN_SCORE`], beats the runner-up by
/// [`DISTINCTIVENESS_MARGIN`], and shares at least one distinctive token.
pub fn score_prompt_against_skills(prompt: &str, skills: &[SkillTerms]) -> Option<SkillMatch> {
    let prompt_tokens = tokenize(prompt);
    if prompt_tokens.is_empty() || skills.is_empty() {
        return None;
    }

    // Document frequency: how many skills contain each token.
    let mut document_frequency: HashMap<&str, usize> = HashMap::new();
    for skill in skills {
        for token in &skill.all_tokens {
            *document_frequency.entry(token.as_str()).or_insert(0) += 1;
        }
    }
    let corpus_size = skills.len() as f64;

    let idf = |token: &str| -> f64 {
        let df = document_frequency.get(token).copied().unwrap_or(0);
        if df == 0 {
            0.0
        } else {
            (corpus_size / df as f64).ln()
        }
    };

    let mut scored: Vec<(usize, f64, bool)> = Vec::with_capacity(skills.len());
    for (index, skill) in skills.iter().enumerate() {
        let mut score = 0.0;
        let mut has_distinctive = false;
        for token in &prompt_tokens {
            if !skill.all_tokens.contains(token) {
                continue;
            }
            let mut weight = idf(token);
            if skill.name_tokens.contains(token) {
                weight *= NAME_TOKEN_BOOST;
            }
            score += weight;
            let df = document_frequency.get(token.as_str()).copied().unwrap_or(0);
            if df > 0 && df <= DISTINCTIVE_DF_MAX {
                has_distinctive = true;
            }
        }
        scored.push((index, score, has_distinctive));
    }

    // Highest score wins; ties broken by name for determinism.
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| skills[a.0].name.cmp(&skills[b.0].name))
    });

    let (best_index, best_score, best_distinctive) = scored[0];
    let min_score = MIN_SCORE_FACTOR * corpus_size.ln();
    if best_score < min_score || !best_distinctive {
        return None;
    }
    let runner_up = scored.get(1).map(|entry| entry.1).unwrap_or(0.0);
    if runner_up > 0.0 && best_score < runner_up * DISTINCTIVENESS_MARGIN {
        return None;
    }

    Some(SkillMatch {
        name: skills[best_index].name.clone(),
        score: best_score,
    })
}

/// English stopwords plus prompt-generic verbs/nouns that carry no routing
/// signal. Kept small and deliberate — over-pruning would starve the matcher,
/// under-pruning lets boilerplate dominate the score.
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "are", "but", "not", "you", "all", "any", "can", "her", "was", "one",
    "our", "out", "use", "using", "used", "how", "what", "why", "when", "who", "where", "which",
    "with", "this", "that", "these", "those", "from", "have", "has", "had", "will", "would",
    "should", "could", "into", "your", "yours", "their", "them", "then", "than", "they", "some",
    "such", "want", "need", "needs", "make", "made", "help", "please", "lets", "let", "get", "got",
    "give", "add", "added", "adding", "fix", "fixed", "fixing", "set", "run", "running", "code",
    "file", "files", "project", "thing", "things", "work", "working", "about", "also", "just",
    "like", "now", "new", "old", "here", "there", "more", "most", "much", "many", "very", "able",
    "via", "per", "its", "it's",
];

/// Tokenize text into a lowercase set: split on non-alphanumeric, keep tokens
/// of length ≥ 3, drop stopwords and pure numbers. A set (not a bag) so a
/// prompt cannot inflate a score by repeating a word.
pub fn tokenize(text: &str) -> HashSet<String> {
    let stop: HashSet<&str> = STOPWORDS.iter().copied().collect();
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter_map(|raw| {
            let token = raw.trim().to_ascii_lowercase();
            if token.len() < 3 {
                return None;
            }
            if token.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            if stop.contains(token.as_str()) {
                return None;
            }
            Some(token)
        })
        .collect()
}

/// Split a `---\n...\n---\n` leading frontmatter block. Returns the frontmatter
/// body (between the fences), or `None` when the file does not open with a
/// fence. Mirrors the parser in `skill_lint` but returns only the part this
/// module needs.
fn split_frontmatter(text: &str) -> Option<String> {
    let trimmed_start = text.trim_start_matches(['\u{feff}', ' ', '\t', '\n', '\r']);
    if !trimmed_start.starts_with("---") {
        return None;
    }
    let after_open = trimmed_start.split_once('\n').map(|(_, rest)| rest)?;
    let mut frontmatter = String::new();
    let mut closed = false;
    for line in after_open.lines() {
        if line.trim() == "---" {
            closed = true;
            break;
        }
        frontmatter.push_str(line);
        frontmatter.push('\n');
    }
    if closed {
        Some(frontmatter)
    } else {
        None
    }
}

/// Read a top-level `key: value` frontmatter field. Ignores indented lines so a
/// nested mapping value (e.g. the `paths:` list) is not misread as a field.
fn frontmatter_field(frontmatter: &str, key: &str) -> Option<String> {
    for line in frontmatter.lines() {
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        if let Some(colon) = line.find(':') {
            if line[..colon].trim() == key {
                return Some(line[colon + 1..].trim().to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str, description: &str, when_to_use: &str) -> SkillTerms {
        let name_tokens = tokenize(&name.replace('-', " "));
        let mut all_tokens = name_tokens.clone();
        all_tokens.extend(tokenize(description));
        all_tokens.extend(tokenize(when_to_use));
        SkillTerms {
            name: name.to_string(),
            all_tokens,
            name_tokens,
        }
    }

    /// A representative slice of the real corpus so threshold behavior is
    /// tested against realistic IDF, not a toy two-skill set.
    fn sample_corpus() -> Vec<SkillTerms> {
        vec![
            skill(
                "stripe-integration",
                "Designs and audits Stripe integrations: Checkout, Payment Intents, Subscriptions, Webhooks, Connect, refunds, disputes, 3DS SCA flows.",
                "Stripe payment integration, webhook reconciliation, and PCI-scope decisions.",
            ),
            skill(
                "postgres-migration-safety",
                "Plans and reviews PostgreSQL migrations with lock analysis, expand-and-contract sequencing, backfill strategy, and rollback boundaries.",
                "PostgreSQL schema changes, migrations, backfills, and lock-sensitive deploys.",
            ),
            skill(
                "reviewer",
                "Reviews completed implementation work for production readiness: quality, security, correctness, testing, release risk.",
                "Production-readiness review and quality gate after implementation.",
            ),
            skill(
                "react-performance-audit",
                "React render-cost tracing, memoization, bundle-size analysis, list virtualization, Core Web Vitals on React routes.",
                "React performance profiling and render-cost reduction.",
            ),
            skill(
                "websocket-realtime-design",
                "WebSocket, SSE, fan-out, reconnect resume, backpressure, ordering and dedup, auth lifecycle on long-lived connections.",
                "Realtime transport and connection lifecycle design.",
            ),
            skill(
                "git-expert",
                "Safe Git workflow: branching, conflict resolution, history repair, secret cleanup, rebase strategy.",
                "Version control operations and history repair.",
            ),
            skill(
                "security-and-compliance-auditor",
                "Security reviews, threat modeling, compliance SOC2 GDPR, remediation quality.",
                "Security audit and compliance review.",
            ),
        ]
    }

    #[test]
    fn distinctive_domain_prompt_matches_its_skill() {
        let corpus = sample_corpus();
        let result =
            score_prompt_against_skills("add stripe checkout to the billing page", &corpus);
        assert_eq!(
            result.map(|m| m.name),
            Some("stripe-integration".to_string())
        );
    }

    #[test]
    fn postgres_migration_prompt_matches() {
        let corpus = sample_corpus();
        let result = score_prompt_against_skills(
            "I need to add a column and backfill a large postgres table without locking",
            &corpus,
        );
        assert_eq!(
            result.map(|m| m.name),
            Some("postgres-migration-safety".to_string())
        );
    }

    #[test]
    fn websocket_prompt_matches() {
        let corpus = sample_corpus();
        let result = score_prompt_against_skills(
            "design reconnect and backpressure for our websocket fan-out",
            &corpus,
        );
        assert_eq!(
            result.map(|m| m.name),
            Some("websocket-realtime-design".to_string())
        );
    }

    #[test]
    fn generic_prompt_does_not_match() {
        let corpus = sample_corpus();
        // No distinctive domain token — must stay silent and let the caller
        // fall back to the generic reminder.
        assert_eq!(
            score_prompt_against_skills("can you help me write a small function", &corpus),
            None
        );
    }

    #[test]
    fn empty_prompt_does_not_match() {
        let corpus = sample_corpus();
        assert_eq!(score_prompt_against_skills("", &corpus), None);
        assert_eq!(score_prompt_against_skills("   ", &corpus), None);
    }

    #[test]
    fn name_token_in_prompt_is_a_strong_signal() {
        let corpus = sample_corpus();
        let result = score_prompt_against_skills("run the reviewer on this diff", &corpus);
        assert_eq!(result.map(|m| m.name), Some("reviewer".to_string()));
    }

    #[test]
    fn tokenize_drops_stopwords_and_short_tokens() {
        let tokens = tokenize("Add the Stripe webhook to a PCI flow");
        assert!(tokens.contains("stripe"));
        assert!(tokens.contains("webhook"));
        assert!(tokens.contains("pci"));
        assert!(tokens.contains("flow"));
        assert!(!tokens.contains("add")); // stopword
        assert!(!tokens.contains("the")); // stopword
        assert!(!tokens.contains("to")); // length < 3
    }

    #[test]
    fn frontmatter_parsing_extracts_fields() {
        let source = "---\nname: stripe-integration\ndescription: Stripe Checkout and webhooks.\nwhen_to_use: Payments.\npaths:\n  - \"**/*stripe*.ts\"\n---\n# body\n";
        let model = skill_terms_from_source("stripe-integration", source).expect("parse");
        assert_eq!(model.name, "stripe-integration");
        assert!(model.all_tokens.contains("stripe"));
        assert!(model.all_tokens.contains("checkout"));
        assert!(model.all_tokens.contains("payments"));
        // The indented `paths:` list value must not leak in as a token.
        assert!(!model.all_tokens.contains("ts"));
    }

    #[test]
    fn source_without_frontmatter_is_skipped() {
        assert!(skill_terms_from_source("x", "# no frontmatter here\nbody\n").is_none());
    }

    #[test]
    fn ambiguous_tie_does_not_match() {
        // Two skills share the same distinctive token and nothing else — the
        // margin guard must keep the matcher silent rather than coin-flip.
        let corpus = vec![
            skill("alpha-tool", "shared widget handling", ""),
            skill("beta-tool", "shared widget handling", ""),
        ];
        assert_eq!(score_prompt_against_skills("widget", &corpus), None);
    }

    #[test]
    fn inline_brief_includes_description_and_body() {
        let source = "---\nname: stripe-integration\ndescription: Stripe Checkout, webhooks, and PCI scope.\n---\n# Stripe integration\n\nAlways verify webhook signatures before trusting the event.\n";
        let brief = inline_brief_from_source(source).expect("brief");
        assert!(brief.contains("Stripe Checkout, webhooks, and PCI scope."));
        assert!(brief.contains("verify webhook signatures"));
        // The frontmatter fence itself must not leak into the brief.
        assert!(!brief.contains("---"));
        assert!(!brief.contains("name: stripe-integration"));
    }

    #[test]
    fn inline_brief_truncates_long_body_on_line_boundary() {
        let mut source = String::from("---\ndescription: D.\n---\n");
        // Build a body well over the cap out of distinct numbered lines.
        for n in 0..400 {
            source.push_str(&format!("line {n} with enough text to add bytes\n"));
        }
        let brief = inline_brief_from_source(&source).expect("brief");
        assert!(
            brief.len() <= INLINE_BRIEF_MAX_BYTES + 120,
            "brief length {} exceeds cap + marker allowance",
            brief.len()
        );
        assert!(brief.contains("[skill brief truncated"));
        // Truncation lands on a line boundary: the last content line before the
        // marker is a whole "line N ..." line, never a fragment.
        let before_marker = brief.split("\n\n[skill brief truncated").next().unwrap();
        assert!(before_marker.ends_with("add bytes"));
    }

    #[test]
    fn inline_brief_none_without_content() {
        // Frontmatter only, empty body, blank description → nothing to inject.
        assert_eq!(inline_brief_from_source("---\ndescription:\n---\n"), None);
        assert_eq!(inline_brief_from_source(""), None);
    }

    #[test]
    fn strip_frontmatter_block_returns_body_after_fence() {
        let body = strip_frontmatter_block("---\nname: x\n---\nhello body\n");
        assert_eq!(body.trim(), "hello body");
        // No frontmatter → whole text is the body.
        assert_eq!(strip_frontmatter_block("plain body").trim(), "plain body");
        // Unterminated frontmatter → no usable body.
        assert_eq!(strip_frontmatter_block("---\nname: x\nno close").trim(), "");
    }

    #[test]
    fn skill_inline_brief_rejects_path_traversal() {
        let dir = std::env::temp_dir();
        assert_eq!(skill_inline_brief(&dir, "../escape"), None);
        assert_eq!(skill_inline_brief(&dir, "a/b"), None);
        assert_eq!(skill_inline_brief(&dir, ""), None);
    }
}
