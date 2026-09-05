//! Purpose: Behavioral skill-activation eval — drives the REAL matcher
//!   (`resolve_skill_for_prompt`) over curated prompt fixtures and asserts each
//!   prompt activates the skill it should (should-trigger) or stays silent on a
//!   generic prompt (should-NOT-trigger). The deterministic, model-free analog of
//!   an LLM-judge eval: reproducible run-to-run, free, and CI-gateable.
//! Caller: commands.rs `skill-eval` dispatch.
//! Dependencies: utility::skill_match (the production matcher), args, json, runtime.
//! Side Effects: reads installed `<name>/SKILL.md` files; writes a report.
//!
//! Why this is "ahead" of an LLM-judge eval: wshobson's plugin-eval rates skills
//! with an LLM (stochastic, needs a model, not reproducible). The property that
//! actually matters for a skill is whether it *fires when it should and stays
//! quiet when it should not* — a behavioral fact the real IDF matcher decides
//! deterministically. So this harness drives the genuine `resolve_skill_for_prompt`
//! over fixtures and asserts the activation decision: same answer every run, no
//! model, gate-able in CI. `skill-lint` checks a skill is well-FORMED; this checks
//! it actually TRIGGERS.

use std::io::Write;

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::runtime::{
    display_path, resolve_claude_home, resolve_repository_root, skills_directory,
};
use crate::utility::skill_match::{load_skill_terms, resolve_skill_for_prompt};

/// One behavioral expectation: a prompt and the set of skills any of which is an
/// acceptable activation, or an empty set for a generic prompt that must NOT trip
/// any skill (the precision side of the eval — over-eager activation is as wrong
/// as a miss).
///
/// `accept` is a SET, not a single name, on purpose: some asks legitimately route
/// to one of several synonymous skills (e.g. "review this diff" is correctly
/// served by either `reviewer` or its alias `requesting-code-review`). Asserting
/// one exact name would make the eval fail on a *correct* match — so the harness
/// expresses "any of these is right" and stays honest rather than over-strict.
struct TriggerFixture {
    prompt: &'static str,
    /// Non-empty = must activate one of these skills; empty = must stay silent.
    accept: &'static [&'static str],
}

/// Embedded fixtures over skills that ship in this repo. Two classes:
///
/// - should-trigger: a realistic on-topic prompt that must resolve to a
///   specific skill (recall, the IDF or curated tier deciding).
/// - should-NOT-trigger: a generic engineering prompt with no distinctive
///   vocabulary, which must resolve to `None` so generic asks are not
///   hijacked by a skill.
///
/// Kept deliberately conservative: every should-trigger prompt names vocabulary
/// distinctive to its target skill, and every should-NOT prompt is ordinary
/// prose. New skills can add a fixture here; the harness then guards that the
/// skill keeps triggering as the corpus grows around it.
const TRIGGER_FIXTURES: &[TriggerFixture] = &[
    TriggerFixture {
        prompt: "add a stripe checkout flow with webhook signature verification to billing",
        accept: &["stripe-integration"],
    },
    TriggerFixture {
        prompt: "design a postgres migration to add a column without locking the table",
        accept: &["postgres-migration-safety"],
    },
    TriggerFixture {
        prompt: "audit our oauth and oidc authentication and token refresh rotation",
        accept: &["security-and-compliance-auditor"],
    },
    TriggerFixture {
        prompt: "design the websocket realtime connection lifecycle and reconnection backoff",
        accept: &["websocket-realtime-design"],
    },
    TriggerFixture {
        // "review this diff" legitimately routes to either the reviewer skill or
        // its alias requesting-code-review — both are correct, so accept either.
        prompt: "review this diff for production readiness before we merge",
        accept: &["reviewer", "requesting-code-review"],
    },
    TriggerFixture {
        prompt: "critique this implementation and tell me what am i missing",
        accept: &["critic"],
    },
    TriggerFixture {
        prompt: "i need some critics about the current codebases",
        accept: &["critic"],
    },
    TriggerFixture {
        prompt: "stress-test this approach before I go further",
        accept: &["critic"],
    },
    TriggerFixture {
        prompt: "the test is failing intermittently, help me find the root cause",
        accept: &["systematic-debugging"],
    },
    TriggerFixture {
        prompt: "use anvil to implement this feature under a named bar",
        accept: &["running-anvil"],
    },
    // UI visual-craft: must activate the UI specialist (not silent IDF ties).
    TriggerFixture {
        prompt: "build a landing page for my beauty spa",
        accept: &["ui-design-systems-and-responsive-interfaces"],
    },
    TriggerFixture {
        prompt: "create a dashboard for healthcare analytics with clear visual hierarchy",
        accept: &["ui-design-systems-and-responsive-interfaces"],
    },
    TriggerFixture {
        prompt: "make this React page look better with glassmorphism and visual polish",
        accept: &["ui-design-systems-and-responsive-interfaces"],
    },
    TriggerFixture {
        prompt: "fix the contrast and focus states on this button for accessible ui",
        accept: &["ui-design-systems-and-responsive-interfaces"],
    },
    TriggerFixture {
        prompt: "choose a color palette and typography for the product dashboard layout",
        accept: &["ui-design-systems-and-responsive-interfaces"],
    },
    TriggerFixture {
        prompt: "fix flutter widget rebuild jank with riverpod state management and platform channels",
        accept: &["dart-and-flutter-expert"],
    },
    TriggerFixture {
        prompt: "report memory health learnings and mistake ledger for this session",
        accept: &["memory-status-reporter"],
    },
    TriggerFixture {
        prompt: "run a red green refactor loop with failing test first for the checkout validator",
        accept: &["test-driven-development"],
    },
    TriggerFixture {
        prompt: "complex git history surgery with interactive rebase reflog recovery and cherry pick",
        accept: &["git-expert"],
    },
    TriggerFixture {
        prompt: "sequence a cross domain delivery plan with architecture framing and milestone coordination",
        accept: &["software-development-life-cycle"],
    },
    TriggerFixture {
        prompt: "handle android activity lifecycle with offline sync and push notification permissions",
        accept: &["mobile-development-life-cycle"],
    },
    TriggerFixture {
        prompt: "improve web vitals with seo meta tags and browser caching for the marketing site",
        accept: &["web-development-life-cycle"],
    },
    TriggerFixture {
        prompt: "rightsize overprovisioned kubernetes nodes and compare reserved versus spot commitments",
        accept: &["cloud-cost-and-finops"],
    },
    TriggerFixture {
        prompt: "extract message catalogs with icu plurals and rtl layout for arabic locale",
        accept: &["internationalization-and-localization"],
    },
    TriggerFixture {
        prompt: "trace react rerenders and cut bundle size with virtualization for a long list",
        accept: &["react-performance-audit"],
    },
    TriggerFixture {
        prompt: "trace ownership of the checkout handler and its callers before editing existing behavior",
        accept: &["preserve-existing-flow"],
    },
    TriggerFixture {
        prompt: "fan out independent migrations to parallel agents sharing no files",
        accept: &["dispatching-parallel-agents"],
    },
    TriggerFixture {
        prompt: "address the review comments point by point with evidence for each fix",
        accept: &["receiving-code-review"],
    },
    TriggerFixture {
        prompt: "two senior engineers disagree on sync versus async at the payment boundary; adjudicate the tradeoff with a scored analysis",
        accept: &["deliberation"],
    },
    TriggerFixture {
        prompt: "design microservice boundaries with message queues cache invalidation and read replicas",
        accept: &["backend-and-data-architecture"],
    },
    TriggerFixture {
        prompt: "define bounded contexts aggregates and ubiquitous language for orders inventory and shipping",
        accept: &["domain-driven-design"],
    },
    // should-NOT-trigger: ordinary requests with no distinctive skill vocabulary.
    TriggerFixture {
        prompt: "can you help me write a small helper function",
        accept: &[],
    },
    TriggerFixture {
        // A clearly out-of-domain prompt: no software-engineering vocabulary at
        // all, so no skill should claim it.
        prompt: "remind me to call my mother this weekend",
        accept: &[],
    },
    // Adversarial should-NOT-trigger (findings #17/#18): these previously
    // mis-routed via a curated substring false-positive or a learned-skill
    // own-name hijack. They must now stay silent (or at least never route to the
    // named wrong skill). Word-boundary curated matching and learned-skill
    // exclusion are what keep them quiet.
    TriggerFixture {
        // `slo` inside "slow" used to route to observability-and-incident-response.
        prompt: "the page is loading slow, can you take a look",
        accept: &[],
    },
    TriggerFixture {
        // `sli` inside "slideshow"/"slightly"/"slicker" used to route to
        // observability-and-incident-response.
        prompt: "make the slideshow transition slightly slicker",
        accept: &[],
    },
    TriggerFixture {
        // With the installed corpus present, `rust` used to hijack learned-rust
        // via the own-name boost. Must never route to a learned-<project> skill;
        // stays silent in the repo corpus (which has no learned skills) too.
        prompt: "fix this rust borrow checker error",
        accept: &[],
    },
    TriggerFixture {
        // Bare "disagree" with no adjudication shape must not trip the
        // deliberation curated phrases.
        prompt: "i disagree with the project timeline",
        accept: &[],
    },
    TriggerFixture {
        // Bare "deliberate" with no between-phrase must not trip deliberation.
        prompt: "was the five second timeout in this function deliberate",
        accept: &[],
    },
];

pub fn run_skill_eval_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("skill-eval");
    flag_set.string_flag("repo-root", "");
    flag_set.string_flag("claude-home", "");
    flag_set.bool_flag("installed", false);
    flag_set.bool_flag("json", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }

    // By default the eval reads the repo's `<name>/SKILL.md` sources (the CI
    // corpus). `--installed` instead reads the installed `<claude_home>/skills`
    // corpus — the one production `match_skill_for_prompt` actually matches
    // against, which includes the `learned-<project>` skills the repo does not
    // ship. That is the corpus where findings #17/#18 are observable end-to-end;
    // the repo corpus cannot see the learned-skill hijack because it has none.
    let corpus_dir = if flag_set.bool_value("installed") {
        let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
            Ok(path) => path,
            Err(error) => {
                let _ = writeln!(standard_error, "skill-eval: {error}");
                return 1;
            }
        };
        skills_directory(&claude_home)
    } else {
        match resolve_repository_root(flag_set.string_value("repo-root")) {
            Ok(path) => path,
            Err(error) => {
                let _ = writeln!(standard_error, "skill-eval: {error}");
                return 1;
            }
        }
    };

    // The corpus dir is the skills dir: each `<name>/SKILL.md` sits one level
    // down, which is exactly the layout load_skill_terms expects.
    let skills = load_skill_terms(&corpus_dir);
    if skills.is_empty() {
        let _ = writeln!(
            standard_error,
            "skill-eval: no skills found under {}",
            display_path(&corpus_dir)
        );
        return 1;
    }

    let mut results: Vec<TriggerResult> = Vec::with_capacity(TRIGGER_FIXTURES.len());
    for fixture in TRIGGER_FIXTURES {
        // Skip a should-trigger fixture when NONE of its acceptable skills are
        // installed in this corpus, so a trimmed install does not fail the eval
        // on a skill it deliberately omitted. A should-NOT fixture (empty accept
        // set) always runs — it asserts silence, which holds regardless of which
        // skills are present.
        if !fixture.accept.is_empty()
            && !fixture
                .accept
                .iter()
                .any(|name| skills.iter().any(|skill| &skill.name == name))
        {
            results.push(TriggerResult {
                prompt: fixture.prompt,
                accept: fixture.accept,
                actual: None,
                outcome: Outcome::Skipped,
            });
            continue;
        }
        let actual = resolve_skill_for_prompt(fixture.prompt, &skills).map(|m| m.name);
        let outcome = classify(fixture.accept, actual.as_deref());
        results.push(TriggerResult {
            prompt: fixture.prompt,
            accept: fixture.accept,
            actual,
            outcome,
        });
    }

    let passed = results
        .iter()
        .filter(|r| r.outcome == Outcome::Pass)
        .count();
    let failed = results
        .iter()
        .filter(|r| r.outcome == Outcome::Fail)
        .count();
    let skipped = results
        .iter()
        .filter(|r| r.outcome == Outcome::Skipped)
        .count();

    if flag_set.bool_value("json") {
        let payload = Value::Object(vec![
            ("total".into(), Value::Number(results.len().to_string())),
            ("passed".into(), Value::Number(passed.to_string())),
            ("failed".into(), Value::Number(failed.to_string())),
            ("skipped".into(), Value::Number(skipped.to_string())),
            (
                "fixtures".into(),
                Value::Array(results.iter().map(result_to_value).collect()),
            ),
        ]);
        let exit = if write_indented(standard_output, &payload).is_err() {
            1
        } else {
            0
        };
        return if failed > 0 { 1 } else { exit };
    }

    let _ = writeln!(
        standard_output,
        "skill-eval: {} fixture(s), {passed} passed, {failed} failed, {skipped} skipped",
        results.len()
    );
    for result in &results {
        let tag = match result.outcome {
            Outcome::Pass => "ok",
            Outcome::Fail => "FAIL",
            Outcome::Skipped => "skip",
        };
        let expectation = if result.accept.is_empty() {
            "stay silent".to_string()
        } else {
            format!("trigger one of [{}]", result.accept.join(", "))
        };
        let got = match &result.actual {
            Some(name) => name.as_str(),
            None => "(silent)",
        };
        let _ = writeln!(
            standard_output,
            "  [{tag}] expect {expectation} | got {got} | {:?}",
            result.prompt
        );
    }
    if failed > 0 {
        1
    } else {
        0
    }
}

/// The three-way outcome of one fixture: PASS when the prompt activated one of
/// its acceptable skills, PASS when an empty accept-set stayed silent, FAIL
/// otherwise (wrong skill, an unexpected activation, or a missed one). Shared by
/// the production loop and the tests so they can never classify differently.
fn classify(accept: &[&str], actual: Option<&str>) -> Outcome {
    match actual {
        // Activated something: pass only if it is an accepted skill.
        Some(got) if accept.contains(&got) => Outcome::Pass,
        Some(_) => Outcome::Fail,
        // Stayed silent: pass only if silence was expected (empty accept set).
        None if accept.is_empty() => Outcome::Pass,
        None => Outcome::Fail,
    }
}

#[derive(Debug, PartialEq)]
enum Outcome {
    Pass,
    Fail,
    Skipped,
}

struct TriggerResult {
    prompt: &'static str,
    accept: &'static [&'static str],
    actual: Option<String>,
    outcome: Outcome,
}

fn result_to_value(result: &TriggerResult) -> Value {
    Value::Object(vec![
        ("prompt".into(), Value::String(result.prompt.to_string())),
        (
            "accept".into(),
            Value::Array(
                result
                    .accept
                    .iter()
                    .map(|name| Value::String(name.to_string()))
                    .collect(),
            ),
        ),
        (
            "actual".into(),
            match &result.actual {
                Some(name) => Value::String(name.clone()),
                None => Value::String(String::new()),
            },
        ),
        (
            "outcome".into(),
            Value::String(
                match result.outcome {
                    Outcome::Pass => "pass",
                    Outcome::Fail => "fail",
                    Outcome::Skipped => "skipped",
                }
                .to_string(),
            ),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utility::skill_match::SkillTerms;
    use std::collections::HashSet;

    fn terms(name: &str, words: &[&str]) -> SkillTerms {
        let all: HashSet<String> = words.iter().map(|w| w.to_string()).collect();
        let name_tokens: HashSet<String> = name.split('-').map(|w| w.to_string()).collect();
        SkillTerms {
            name: name.to_string(),
            all_tokens: all.into_iter().chain(name_tokens.clone()).collect(),
            name_tokens,
        }
    }

    #[test]
    fn distinctive_prompt_triggers_its_skill() {
        // A corpus where "stripe"/"webhook" are distinctive to one skill: the
        // prompt must resolve to it through the real matcher.
        let corpus = vec![
            terms(
                "stripe-integration",
                &["stripe", "webhook", "checkout", "billing"],
            ),
            terms(
                "postgres-migration-safety",
                &["postgres", "migration", "column", "lock"],
            ),
            terms("reviewer", &["review", "production", "readiness"]),
        ];
        let got = resolve_skill_for_prompt("add stripe webhook checkout", &corpus);
        assert_eq!(got.map(|m| m.name).as_deref(), Some("stripe-integration"));
    }

    #[test]
    fn generic_prompt_stays_silent() {
        // The precision side: an ordinary prompt with no distinctive vocabulary
        // must resolve to None, not hijack a skill.
        let corpus = vec![
            terms("stripe-integration", &["stripe", "webhook", "checkout"]),
            terms(
                "postgres-migration-safety",
                &["postgres", "migration", "column"],
            ),
        ];
        let got = resolve_skill_for_prompt("please write a small helper function", &corpus);
        assert!(
            got.is_none(),
            "generic prompt should stay silent, got {got:?}"
        );
    }

    #[test]
    fn adversarial_prompts_stay_silent_over_installed_shaped_corpus() {
        // Findings #17/#18 are only observable against a corpus that carries the
        // curated-tier target (observability) and a learned-<project> skill —
        // exactly the installed corpus, which the repo corpus lacks. Build that
        // shape and drive the REAL resolver: the substring false-positives and the
        // learned-skill hijack must all stay silent / never route to the wrong
        // skill.
        let corpus = vec![
            terms(
                "observability-and-incident-response",
                &["telemetry", "alerting", "paging", "runbook", "incident"],
            ),
            terms(
                "learned-rust",
                &["learned", "procedures", "project", "observed", "patterns"],
            ),
            terms("reviewer", &["review", "production", "readiness"]),
        ];
        // #17: `slo`/`sli` embedded in ordinary words must not route.
        assert_eq!(
            resolve_skill_for_prompt("the page is loading slow, can you take a look", &corpus)
                .map(|m| m.name),
            None,
            "`slo` inside `slow` must not route to observability"
        );
        assert_eq!(
            resolve_skill_for_prompt("make the slideshow transition slightly slicker", &corpus)
                .map(|m| m.name),
            None,
            "`sli` inside slideshow must not route to observability"
        );
        // #18: a bare language token must never route to a learned-<project> skill.
        assert_ne!(
            resolve_skill_for_prompt("fix this rust borrow checker error", &corpus).map(|m| m.name),
            Some("learned-rust".to_string()),
            "`rust` must not hijack learned-rust"
        );
        // Positive control: a genuine standalone `slo` still routes to observability.
        assert_eq!(
            resolve_skill_for_prompt("define an slo and error budget for the service", &corpus)
                .map(|m| m.name),
            Some("observability-and-incident-response".to_string()),
            "standalone `slo` must still route"
        );
    }

    #[test]
    fn outcome_classification_matches_expectation() {
        // The three-way outcome logic against the SHARED production classify():
        // pass on an accepted activation, pass on expected silence, fail on a
        // wrong skill, an unexpected activation, or a missed one.
        assert_eq!(classify(&["a"], Some("a")), Outcome::Pass);
        assert_eq!(classify(&["a", "b"], Some("b")), Outcome::Pass); // synonym set
        assert_eq!(classify(&[], None), Outcome::Pass); // expected silence
        assert_eq!(classify(&["a"], Some("b")), Outcome::Fail); // wrong skill
        assert_eq!(classify(&["a"], None), Outcome::Fail); // missed activation
        assert_eq!(classify(&[], Some("a")), Outcome::Fail); // unexpected activation
    }
}
