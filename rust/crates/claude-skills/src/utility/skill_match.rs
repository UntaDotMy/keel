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

use crate::runtime::{safe_path_segment, skills_directory};

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
    if let Some(found) = score_prompt_against_skills(prompt, &skills) {
        return Some(found);
    }
    // The IDF matcher stayed silent (no corpus-rare token). Before giving up,
    // try the curated cross-cutting tier: a small set of always-applies skills
    // (reviewer, TDD, debugging, preserve-existing-flow) whose vocabulary is
    // high-frequency across the corpus and therefore never distinctive, even
    // though the prompt clearly calls for them. Only route to a curated skill
    // that is actually installed, so a trimmed install never points at a
    // missing skill.
    let curated = curated_skill_for_prompt(prompt)?;
    if skills.iter().any(|skill| skill.name == curated) {
        return Some(SkillMatch {
            name: curated.to_string(),
            // Sentinel score: curated matches are keyword-routed, not IDF-scored.
            // The caller only uses the name, so the exact value is immaterial;
            // 0.0 documents "did not clear the statistical bar."
            score: 0.0,
        });
    }
    None
}

/// Curated cross-cutting skill triggers, evaluated only when the statistical
/// matcher is silent.
///
/// These skills apply to a whole *class* of work but use high-frequency verbs
/// (review, test, debug, edit) — or, for the UI/UX pair, share nearly all their
/// vocabulary with each other — so the IDF matcher correctly treats them as
/// non-distinctive and stays silent, leaving the most-used everyday skills with
/// no inline brief. Each entry lists narrow, verb- or noun-anchored trigger
/// phrases; the first skill whose phrase appears in the (lowercased) prompt
/// wins. Phrases are intentionally specific so ordinary prose ("I reviewed the
/// docs") does not trip them.
///
/// Order matters: earlier entries win ties. `systematic-debugging` precedes
/// `test-driven-development` because "the test is failing" is a debugging ask,
/// not a request to author a new test. The UI and UX phrase sets are kept
/// disjoint (visual-craft vs. research/flow) so the two never collide.
const CURATED_SKILL_TRIGGERS: &[(&str, &[&str])] = &[
    (
        "reviewer",
        &[
            "review this diff",
            "review the diff",
            "review this pr",
            "review the pr",
            "review this code",
            "review the code",
            "review my code",
            "review my changes",
            "review the changes",
            "code review",
            "ready to merge",
            "ready for merge",
            "before we merge",
            "production ready",
            "production readiness",
            "release readiness",
            "is this ready to ship",
        ],
    ),
    (
        "systematic-debugging",
        &[
            "test is failing",
            "tests are failing",
            "this test is failing",
            "it keeps failing",
            "keeps crashing",
            "flaky test",
            "flaky tests",
            "intermittent failure",
            "debug this",
            "debug the",
            "why is this failing",
            "why does this fail",
            "track down the bug",
            "find the root cause",
            "reproduce the bug",
        ],
    ),
    (
        "test-driven-development",
        &[
            "write a test",
            "write tests",
            "add a test",
            "add tests",
            "add unit tests",
            "test first",
            "test-driven",
            "tdd",
            "red green refactor",
            "cover this with tests",
            "write a failing test",
        ],
    ),
    (
        "preserve-existing-flow",
        &[
            "edit the existing",
            "change the existing",
            "modify the existing",
            "refactor the existing",
            "change existing behavior",
            "before editing",
            "before i edit",
            "without breaking existing",
            "don't break existing",
        ],
    ),
    // UI visual-craft asks. The two UI/UX skills share almost all their
    // vocabulary (design, ui, ux, interface, experience), so the IDF matcher
    // ties on them and correctly stays silent — leaving obvious UI prompts with
    // no inline guidance (the real-world failure that produced "I don't have a
    // UI/UX skill registered"). These phrases are visual-craft-specific so they
    // route to `ui-design-systems-…` without colliding with the research-shaped
    // UX phrases below.
    (
        "ui-design-systems-and-responsive-interfaces",
        &[
            "typography",
            "responsive layout",
            "responsive design",
            "design system",
            "looks ai-generated",
            "look less ai-generated",
            "less ai-generated",
            "ai-generated look",
            "visual hierarchy",
            "visual polish",
            "visual craft",
            "color palette",
            "color discipline",
            "spacing and",
            "make this look",
            "make it look",
            "style the",
            "css layout",
            "tailwind",
            "component library",
            "wcag",
            "accessible ui",
            "ui polish",
            "ui design",
        ],
    ),
    // UX research / experience-strategy asks. Research- and flow-shaped phrasing,
    // kept disjoint from the visual-craft phrases above so the two never tie.
    (
        "ux-research-and-experience-strategy",
        &[
            "user research",
            "usability",
            "user journey",
            "user journeys",
            "user flow",
            "user flows",
            "conversion funnel",
            "drop-off",
            "personas",
            "wireframe",
            "wireframes",
            "information architecture",
            "ux research",
            "experience strategy",
            "user testing",
            "journey map",
        ],
    ),
];

/// Pure curated-tier lookup (no IO) so the trigger phrases are unit-testable.
/// Returns the first curated skill whose trigger phrase appears in the
/// lowercased prompt, or `None`. Conservative by construction: it errs toward
/// silence, and the caller still gates on the skill being installed.
pub fn curated_skill_for_prompt(prompt: &str) -> Option<&'static str> {
    let lowered = prompt.to_ascii_lowercase();
    for (skill, phrases) in CURATED_SKILL_TRIGGERS {
        if phrases.iter().any(|phrase| lowered.contains(phrase)) {
            return Some(skill);
        }
    }
    None
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
    let skill_path = resolve_skill_path(claude_home, skill_name)?;
    let text = fs::read_to_string(&skill_path).ok()?;
    inline_brief_from_source(&text)
}

/// Resolve `<skills_dir>/<name>/SKILL.md` for an installed skill, rejecting any
/// `name` that could traverse outside the skills directory or reach a reserved
/// directory. `skill_name` usually comes from a matched installed skill (the
/// matcher only returns names it read from a real directory), but the guard is
/// defensive so a crafted frontmatter `name` — or a caller-supplied name from
/// the MCP surface — can never escape. Delegates the segment-safety check to
/// `runtime::safe_path_segment`, which rejects separators, `.`/`..`, absolute
/// paths, and Windows drive-relative prefixes (`C:foo`) via the OS path parser.
/// Also rejects the `_`/`.`-prefixed reserved directories (`_shared`, hidden)
/// that the discovery traversal hides, so the get/route surface admits exactly
/// the set the catalog enumerates.
fn resolve_skill_path(claude_home: &Path, skill_name: &str) -> Option<std::path::PathBuf> {
    let segment = safe_path_segment(skill_name)?;
    if segment.starts_with('_') || segment.starts_with('.') {
        return None;
    }
    Some(skills_directory(claude_home).join(segment).join("SKILL.md"))
}

/// Read the full SKILL.md body for an installed skill. Returns the resolved
/// path and the complete file text (frontmatter included) so MCP callers can
/// pull the entire skill on demand — the `skill_get` tool's backing — without
/// the brief truncation `skill_inline_brief` applies. `None` when the name is
/// unsafe or the file is missing/unreadable.
pub fn skill_full_body(
    claude_home: &Path,
    skill_name: &str,
) -> Option<(std::path::PathBuf, String)> {
    let skill_path = resolve_skill_path(claude_home, skill_name)?;
    let text = fs::read_to_string(&skill_path).ok()?;
    Some((skill_path, text))
}

/// One installed skill's catalog row: its directory name (the resolve key for
/// `skill_get`/`skill_route`) plus the two frontmatter fields the Claude Code
/// matcher reads. Backs the MCP `skill_list` tool.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillCatalogEntry {
    pub name: String,
    pub description: String,
    pub when_to_use: String,
}

/// Enumerate every installed skill under `<claude_home>/skills`, returning the
/// name + `description` + `when_to_use` frontmatter for each. Skips `_shared`,
/// hidden directories, and any directory without a parseable SKILL.md — the
/// same traversal `load_skill_terms` uses. Sorted by name for stable output.
pub fn skill_catalog(claude_home: &Path) -> Vec<SkillCatalogEntry> {
    let skills_dir = skills_directory(claude_home);
    let entries = match fs::read_dir(&skills_dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut catalog = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        if dir_name.starts_with('.') || dir_name.starts_with('_') {
            continue;
        }
        let skill_path = entry.path().join("SKILL.md");
        let text = match fs::read_to_string(&skill_path) {
            Ok(text) => text,
            Err(_) => continue,
        };
        let Some(frontmatter) = split_frontmatter(&text) else {
            continue;
        };
        let description = frontmatter_field(&frontmatter, "description").unwrap_or_default();
        let when_to_use = frontmatter_field(&frontmatter, "when_to_use").unwrap_or_default();
        // `name` is the directory name — the key `skill_get`/`skill_route`
        // resolve against — so a `skill_list` entry always round-trips back
        // through `skill_get`. The frontmatter `name` is display metadata only.
        catalog.push(SkillCatalogEntry {
            name: dir_name,
            description,
            when_to_use,
        });
    }
    catalog.sort_by(|left, right| left.name.cmp(&right.name));
    catalog
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
/// Uses the frontmatter `description` + `when_to_use` plus the name as the
/// matchable surface — exactly the fields the Claude Code matcher reads.
///
/// The returned `name` is the **directory name**, not the frontmatter `name`.
/// `name` is the stable resolve key: `match_skill_for_prompt` hands it to
/// `skill_inline_brief`/`skill_get`, which resolve `<skills>/<name>/SKILL.md` —
/// a directory join. Keying on the frontmatter name would break that join for
/// any skill whose frontmatter name differs from its directory. Tokens from
/// both the directory name and the frontmatter name feed the matchable surface
/// so divergence never costs match signal.
fn skill_terms_from_source(dir_name: &str, text: &str) -> Option<SkillTerms> {
    let frontmatter = split_frontmatter(text)?;
    let description = frontmatter_field(&frontmatter, "description").unwrap_or_default();
    let when_to_use = frontmatter_field(&frontmatter, "when_to_use").unwrap_or_default();
    let frontmatter_name = frontmatter_field(&frontmatter, "name")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| dir_name.to_string());

    // Tokenize both the directory name and the frontmatter name so the name
    // signal is preserved when they diverge; the resolve key stays the dir name.
    let mut name_tokens = tokenize(&dir_name.replace('-', " "));
    name_tokens.extend(tokenize(&frontmatter_name.replace('-', " ")));
    let mut all_tokens = name_tokens.clone();
    all_tokens.extend(tokenize(&description));
    all_tokens.extend(tokenize(&when_to_use));

    if all_tokens.is_empty() {
        return None;
    }
    Some(SkillTerms {
        name: dir_name.to_string(),
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
            let is_own_name_token = skill.name_tokens.contains(token);
            // A skill's own name token is its distinctive handle. Sibling
            // skills that mention it by name inflate its corpus document
            // frequency, which would otherwise crush its IDF (ln(N/df)) toward
            // zero and drop the match below the score floor — the RC7 bug where
            // a skill became unreachable by its own name. For the owning skill
            // we therefore score the token as if it were unique (df=1, i.e.
            // ln(N)) and apply the name boost, so naming a skill always reaches
            // it regardless of how many siblings reference it. Non-name tokens
            // use the real corpus IDF.
            let weight = if is_own_name_token {
                corpus_size.ln() * NAME_TOKEN_BOOST
            } else {
                idf(token)
            };
            score += weight;
            // Distinctive if corpus-rare OR this skill's own name token (same
            // principle as the score: naming a skill is distinctive to it).
            let df = document_frequency.get(token.as_str()).copied().unwrap_or(0);
            if is_own_name_token || (df > 0 && df <= DISTINCTIVE_DF_MAX) {
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

    #[test]
    fn resolve_skill_path_rejects_traversal_and_reserved_names() {
        let home = Path::new("/fake/home");
        // Traversal / separator / absolute / drive-relative names must yield None.
        for evil in [
            "",
            "   ",
            "..",
            "../evil",
            "../../etc",
            "a/b",
            "a\\b",
            "/abs",
            "C:foo",
            "C:",
        ] {
            assert!(
                resolve_skill_path(home, evil).is_none(),
                "name {evil:?} must be rejected"
            );
        }
        // Reserved/hidden directories the catalog hides are not gettable either.
        assert!(resolve_skill_path(home, "_shared").is_none());
        assert!(resolve_skill_path(home, ".hidden").is_none());
        // An ordinary skill name resolves to <skills>/<name>/SKILL.md.
        let ok = resolve_skill_path(home, "reviewer").expect("ordinary name resolves");
        assert!(ok.ends_with("reviewer/SKILL.md") || ok.ends_with("reviewer\\SKILL.md"));
    }

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
    fn own_name_token_stays_distinctive_despite_cross_references() {
        // RC7 regression: when many other skills mention a skill by name in
        // their descriptions, that name token's document frequency climbs past
        // DISTINCTIVE_DF_MAX, so the rarity gate alone would stop the skill from
        // matching its own handle. The own-name exemption must keep it
        // reachable. Build a corpus where "reviewer" appears in 4 skills (df=4
        // > 3) but only the reviewer skill has it as a *name* token.
        let corpus = vec![
            skill(
                "reviewer",
                "Production-readiness review and quality gate.",
                "Review a diff before merge.",
            ),
            skill(
                "test-driven-development",
                "RED-GREEN-REFACTOR; complements the reviewer and qa work.",
                "Write a failing test first.",
            ),
            skill(
                "finishing-a-development-branch",
                "Close out a branch: verify, then route non-trivial work through the reviewer.",
                "Branch closeout.",
            ),
            skill(
                "receiving-code-review",
                "Act on reviewer feedback as the author.",
                "Address review comments.",
            ),
        ];
        // df("reviewer") == 4 here, above the rarity gate, yet naming it must
        // still reach the reviewer skill via the own-name exemption.
        let result = score_prompt_against_skills("have the reviewer look at this", &corpus);
        assert_eq!(result.map(|m| m.name), Some("reviewer".to_string()));
    }

    #[test]
    fn curated_tier_routes_cross_cutting_prompts() {
        // RC2: these verb-anchored prompts carry no corpus-rare token, so the
        // IDF matcher stays silent — the curated tier is what routes them.
        assert_eq!(
            curated_skill_for_prompt("please review this diff"),
            Some("reviewer")
        );
        assert_eq!(
            curated_skill_for_prompt("this test is failing intermittently"),
            Some("systematic-debugging")
        );
        assert_eq!(
            curated_skill_for_prompt("write a test for the parser"),
            Some("test-driven-development")
        );
        assert_eq!(
            curated_skill_for_prompt("I need to modify the existing auth handler"),
            Some("preserve-existing-flow")
        );
    }

    #[test]
    fn curated_tier_prefers_debugging_over_tdd_for_failing_tests() {
        // "the test is failing" is a debugging ask, not a request to author a
        // new test — debugging must win the tie via trigger order.
        assert_eq!(
            curated_skill_for_prompt("the test is failing, help me fix it"),
            Some("systematic-debugging")
        );
    }

    #[test]
    fn curated_tier_silent_on_ordinary_prose() {
        // Must not trip on incidental mentions — only verb-anchored asks.
        assert_eq!(
            curated_skill_for_prompt("I reviewed the docs yesterday"),
            None
        );
        assert_eq!(curated_skill_for_prompt("add a logout button"), None);
        assert_eq!(curated_skill_for_prompt(""), None);
    }

    #[test]
    fn curated_tier_routes_ui_visual_craft_prompts() {
        // Regression for the real-world failure: the two UI/UX skills share
        // almost all their vocabulary, so the IDF matcher ties and stays silent,
        // and an obvious UI prompt got NO inline guidance — the agent then said
        // "I don't have a UI/UX skill registered." Visual-craft phrasing must now
        // route to ui-design-systems via the curated tier.
        for prompt in [
            "make this dashboard look less ai-generated, improve the typography and spacing",
            "tighten the color discipline and visual hierarchy",
            "add a responsive layout for mobile",
            "build out the design system tokens",
            "make this look more polished",
        ] {
            assert_eq!(
                curated_skill_for_prompt(prompt),
                Some("ui-design-systems-and-responsive-interfaces"),
                "UI visual-craft prompt must route to ui-design-systems: {prompt:?}"
            );
        }
    }

    #[test]
    fn curated_tier_routes_ux_research_prompts() {
        // Research/flow-shaped phrasing routes to ux-research, disjoint from the
        // visual-craft phrases so the two never collide.
        for prompt in [
            "run some user research on the onboarding",
            "map the user journey through checkout",
            "where is the drop-off in the conversion funnel",
            "let's do usability testing on this",
            "sketch a wireframe for the settings page",
        ] {
            assert_eq!(
                curated_skill_for_prompt(prompt),
                Some("ux-research-and-experience-strategy"),
                "UX research prompt must route to ux-research: {prompt:?}"
            );
        }
    }

    #[test]
    fn curated_tier_ui_and_ux_phrase_sets_are_disjoint() {
        // The whole point of splitting them is that no single prompt should
        // satisfy both sets — that disjointness is what stops the original tie
        // from reappearing inside the curated tier. Spot-check the representative
        // phrases do not cross-match.
        assert_eq!(
            curated_skill_for_prompt("improve the typography"),
            Some("ui-design-systems-and-responsive-interfaces")
        );
        assert_eq!(
            curated_skill_for_prompt("map the user journey"),
            Some("ux-research-and-experience-strategy")
        );
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

    #[test]
    fn curated_match_requires_the_skill_to_be_installed() {
        // match_skill_for_prompt gates a curated route on the skill actually
        // existing under <home>/skills/. Build a home whose only installed skill
        // is unrelated, then fire a curated-trigger prompt for a skill that is
        // NOT installed: the IDF matcher is silent and the curated branch must
        // return None rather than point at a missing skill.
        let suffix = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        let home = std::env::temp_dir().join(format!("curated-gate-{suffix}"));
        let only_skill = skills_directory(&home).join("unrelated-skill");
        fs::create_dir_all(&only_skill).unwrap();
        fs::write(
            only_skill.join("SKILL.md"),
            "---\nname: unrelated-skill\ndescription: Something entirely unrelated to reviewing.\nwhen_to_use: Never for reviews.\n---\nbody\n",
        )
        .unwrap();

        // "review this diff" is a curated trigger for `reviewer`, which is not
        // installed here. Must be None, not a dangling pointer.
        assert_eq!(
            match_skill_for_prompt(&home, "please review this diff"),
            None
        );

        let _ = fs::remove_dir_all(&home);
    }
}
