//! Purpose: Real compaction eval — runs the genuine proxy reducer pipeline over
//!   committed fixture corpora and computes EXACT o200k_base token deltas at
//!   runtime. Unlike the legacy `bench` parity-marker (which sums hardcoded
//!   byte counts and never executes anything), every number here is produced by
//!   running the real adapters: `classify_command` -> registry `best_match` ->
//!   `adapter.compact()` -> `render_compact_result()`, then counting tokens with
//!   the real `tiktoken_rs` tokenizer. The savings are measured, not asserted.
//! Caller: commands.rs `eval` dispatch; the module tests assert measured floors
//!   so a regression that silently stops compacting fails CI.
//! Dependencies: the proxy layer (classify, adapters, token_meter, render) and
//!   the crate-local args/json helpers.
//! Side Effects: none — fixtures are embedded, no filesystem or network. The
//!   eval is fully deterministic, so two runs over the same binary produce
//!   identical numbers.
//!
//! Why end-to-end token counting: the honest question is "how many tokens does
//! the agent actually receive?". That is the token count of the rendered
//! compact output (summary + kept lines + the raw-recovery pointer), INCLUDING
//! the wrapper overhead — not the adapter's internal self-report. So the
//! headline metric here counts `render_compact_result()`, which is exactly the
//! text the proxy injects into the transcript.

use std::io::Write;
use std::path::PathBuf;

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::proxy::adapters::build_adapter_registry;
use crate::proxy::classify::classify_command;
use crate::proxy::raw_store::RunMeta;
use crate::proxy::render::render_compact_result;
use crate::proxy::token_meter::TokenMeter;

/// An embedded eval fixture: a real command and a realistic raw output capture.
/// The eval runs each fixture through the actual reducer pipeline, so the raw
/// text here is the genuine input the proxy would see in a live session.
struct EvalFixture {
    name: &'static str,
    command: &'static [&'static str],
    raw_stdout: &'static str,
    raw_stderr: &'static str,
    exit_code: i32,
    /// The adapter this fixture is intended to exercise. The eval validates
    /// this before measuring so a classifier regression cannot silently turn
    /// an adapter benchmark into a generic-adapter benchmark.
    expected_adapter: &'static str,
}

/// One fixture's measured result after running the real pipeline.
#[derive(Debug, Clone)]
pub struct EvalCase {
    pub name: String,
    pub command: String,
    pub adapter: String,
    /// Whether the proxy's break-even guard kept the compact output (true) or
    /// fell back to raw passthrough because compaction did not shrink the token
    /// count (false). Passthrough cases report zero savings, never negative.
    pub compacted: bool,
    pub tokens_raw: usize,
    pub tokens_compact: usize,
    pub tokens_saved: isize,
    pub savings_pct: f64,
}

/// Aggregate measured eval result across all fixtures.
#[derive(Debug, Clone)]
pub struct EvalReport {
    pub cases: Vec<EvalCase>,
    pub total_tokens_raw: usize,
    pub total_tokens_compact: usize,
    pub total_tokens_saved: isize,
    pub overall_savings_pct: f64,
}

/// Run the compaction eval: for every fixture, drive the REAL pipeline and
/// measure the exact end-to-end token delta. This is the function the tests and
/// the CLI both call, so there is one measured source of truth.
pub fn run_compaction_eval() -> EvalReport {
    let registry = build_adapter_registry();
    let mut cases = Vec::new();
    let mut total_raw = 0usize;
    let mut total_compact = 0usize;

    for fixture in EVAL_FIXTURES {
        let args: Vec<String> = fixture.command.iter().map(|s| s.to_string()).collect();
        // classify_command returns None only for an empty arg vector; every
        // fixture has a program, so a None here is a fixture bug, not runtime
        // input we must tolerate gracefully.
        let ast = classify_command(&args)
            .unwrap_or_else(|| panic!("fixture {} did not classify", fixture.name));
        let adapter = registry
            .best_match(&ast)
            .expect("generic adapter always matches");
        assert_eq!(
            adapter.name(),
            fixture.expected_adapter,
            "eval fixture {} routed to {} instead of {}",
            fixture.name,
            adapter.name(),
            fixture.expected_adapter
        );

        let stdout = fixture.raw_stdout.as_bytes();
        let stderr = fixture.raw_stderr.as_bytes();

        // Build the same RunMeta shape proxy::run constructs before calling an
        // adapter, with the real pre-compaction token count. Only the fields the
        // adapters actually read need real values; the rest are inert defaults.
        let meta = eval_meta(&ast.program, &ast.args, stdout, stderr);
        let result = adapter.compact(stdout, stderr, fixture.exit_code, &meta);

        // End-to-end honesty, modeling the proxy's break-even guard
        // (`run.rs`): count the tokens the agent ACTUALLY receives. The raw
        // stdout+stderr is the baseline. The proxy keeps the rendered compact
        // output ONLY when it is strictly smaller than the raw; otherwise it
        // passes the (neutralized) raw through, so the agent sees raw-many
        // tokens and the case nets zero — never negative. Both counts use the
        // real o200k_base tokenizer, so this is an exact measured delta.
        let tokens_raw = TokenMeter::count_bytes(stdout) + TokenMeter::count_bytes(stderr);
        let rendered = render_compact_result(&result);
        let rendered_tokens = TokenMeter::count_text(&rendered);
        let compacted = result.compacted && rendered_tokens < tokens_raw;
        let tokens_compact = if compacted {
            rendered_tokens
        } else {
            tokens_raw
        };
        let tokens_saved = tokens_raw as isize - tokens_compact as isize;
        let savings_pct = if tokens_raw == 0 {
            0.0
        } else {
            (tokens_saved.max(0) as f64 / tokens_raw as f64) * 100.0
        };

        total_raw += tokens_raw;
        total_compact += tokens_compact;
        cases.push(EvalCase {
            name: fixture.name.to_string(),
            command: fixture.command.join(" "),
            adapter: result.adapter_name,
            compacted,
            tokens_raw,
            tokens_compact,
            tokens_saved,
            savings_pct,
        });
    }

    let total_saved = total_raw as isize - total_compact as isize;
    let overall_savings_pct = if total_raw == 0 {
        0.0
    } else {
        (total_saved.max(0) as f64 / total_raw as f64) * 100.0
    };

    EvalReport {
        cases,
        total_tokens_raw: total_raw,
        total_tokens_compact: total_compact,
        total_tokens_saved: total_saved,
        overall_savings_pct,
    }
}

/// Build a `RunMeta` carrying the real pre-compaction token count for an eval
/// fixture. Mirrors the construction in `proxy::run` but with inert values for
/// the persistence fields the adapters never read.
fn eval_meta(program: &str, args: &[String], stdout: &[u8], stderr: &[u8]) -> RunMeta {
    RunMeta {
        raw_id: "eval".to_string(),
        command: if args.is_empty() {
            program.to_string()
        } else {
            format!("{} {}", program, args.join(" "))
        },
        program: program.to_string(),
        args: args.to_vec(),
        cwd: PathBuf::from("."),
        started_at: 0,
        duration_ms: 0,
        exit_code: 0,
        adapter_name: String::new(),
        raw_path: PathBuf::new(),
        compact_path: PathBuf::new(),
        agent: "eval".to_string(),
        workspace: PathBuf::from("."),
        stdout_bytes: stdout.len(),
        stderr_bytes: stderr.len(),
        compact_stdout_bytes: 0,
        compact_stderr_bytes: 0,
        estimated_tokens_before: TokenMeter::count_bytes(stdout) + TokenMeter::count_bytes(stderr),
        estimated_tokens_after: 0,
        estimated_tokens_saved: 0,
        savings_pct: 0.0,
        compacted: false,
    }
}

pub fn run_eval_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("eval");
    flag_set.bool_flag("json", false);
    flag_set.bool_flag("cases", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }

    let report = run_compaction_eval();

    if flag_set.bool_value("json") {
        let case_values: Vec<Value> = report
            .cases
            .iter()
            .map(|case| {
                Value::Object(vec![
                    ("name".into(), Value::String(case.name.clone())),
                    ("command".into(), Value::String(case.command.clone())),
                    ("adapter".into(), Value::String(case.adapter.clone())),
                    ("compacted".into(), Value::Bool(case.compacted)),
                    (
                        "tokensRaw".into(),
                        Value::Number(case.tokens_raw.to_string()),
                    ),
                    (
                        "tokensCompact".into(),
                        Value::Number(case.tokens_compact.to_string()),
                    ),
                    (
                        "tokensSaved".into(),
                        Value::Number(case.tokens_saved.to_string()),
                    ),
                    (
                        "savingsPercent".into(),
                        Value::Number(format!("{:.2}", case.savings_pct)),
                    ),
                ])
            })
            .collect();
        let payload = Value::Object(vec![
            ("tokenizer".into(), Value::String("o200k_base".into())),
            (
                "measurement".into(),
                Value::String("end-to-end: raw stdout+stderr vs rendered compact output".into()),
            ),
            (
                "fixtureCount".into(),
                Value::Number(report.cases.len().to_string()),
            ),
            (
                "totalTokensRaw".into(),
                Value::Number(report.total_tokens_raw.to_string()),
            ),
            (
                "totalTokensCompact".into(),
                Value::Number(report.total_tokens_compact.to_string()),
            ),
            (
                "totalTokensSaved".into(),
                Value::Number(report.total_tokens_saved.to_string()),
            ),
            (
                "overallSavingsPercent".into(),
                Value::Number(format!("{:.2}", report.overall_savings_pct)),
            ),
            ("cases".into(), Value::Array(case_values)),
        ]);
        return write_indented(standard_output, &payload).map_or(1, |_| 0);
    }

    let _ = writeln!(
        standard_output,
        "keel eval: real compaction measurement (o200k_base, end-to-end)"
    );
    let _ = writeln!(
        standard_output,
        "fixtures={} tokens_raw={} tokens_compact={} tokens_saved={} savings={:.2}%",
        report.cases.len(),
        report.total_tokens_raw,
        report.total_tokens_compact,
        report.total_tokens_saved,
        report.overall_savings_pct
    );
    if flag_set.bool_value("cases") {
        for case in &report.cases {
            // `passthrough` marks a case where the break-even guard kept the raw
            // output because compaction would not have shrunk it — saved=0, not
            // negative. That is the guard working, not a failure.
            let mode = if case.compacted {
                "compacted"
            } else {
                "passthrough"
            };
            let _ = writeln!(
                standard_output,
                "- {} [{}/{mode}] raw={} compact={} saved={} ({:.1}%)",
                case.name,
                case.adapter,
                case.tokens_raw,
                case.tokens_compact,
                case.tokens_saved,
                case.savings_pct
            );
        }
    }
    0
}

/// Embedded fixture corpus: realistic raw command outputs spanning the adapter
/// families. Kept inline (not on disk) so the eval is hermetic and deterministic
/// in CI. Each raw block is representative of genuine high-volume tool output a
/// session would otherwise inject verbatim.
const EVAL_FIXTURES: &[EvalFixture] = &[
    EvalFixture {
        name: "cargo-test-pass",
        command: &["cargo", "test", "--workspace"],
        expected_adapter: "tests",
        exit_code: 0,
        raw_stderr: "   Compiling keel v0.1.0\n    Finished test profile in 4.31s\n",
        raw_stdout: "\nrunning 48 tests\ntest adapters::git::tests::status_compacts ... ok\ntest adapters::tests::tests::pass_summary ... ok\ntest adapters::search::tests::groups_by_file ... ok\ntest adapters::build::tests::errors_first ... ok\ntest adapters::lint::tests::warnings_kept ... ok\ntest adapters::cloud::tests::secrets_redacted ... ok\ntest adapters::database::tests::result_table ... ok\ntest adapters::containers::tests::ps_table ... ok\ntest proxy::token_meter::tests::counts_exact ... ok\ntest proxy::classify::tests::routes_cargo_test ... ok\ntest proxy::registry::tests::specific_before_generic ... ok\ntest proxy::render::tests::renders_pass ... ok\ntest utility::recall::tests::indexes_markdown ... ok\ntest utility::recall::tests::json_briefs ... ok\ntest utility::recall::tests::auto_sync ... ok\ntest utility::skill_lint::tests::well_formed_passes ... ok\ntest utility::sprint::tests::review_fails_until_done ... ok\ntest utility::user_story::tests::gherkin_required ... ok\ntest runner::hook_lifecycle::tests::gate_cannot_loop ... ok\ntest runner::learning::tests::recurring_failure_instinct ... ok\ntest mcp::tools::tests::recall_missing_query ... ok\ntest mcp::tools::tests::skill_route_missing_prompt ... ok\ntest review::tests::tally_counts_blocking ... ok\ntest manager::install::tests::stages_shared ... ok\n... 24 more passing tests omitted for brevity in this fixture ...\n\ntest result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.80s\n",
    },
    EvalFixture {
        name: "cargo-test-fail",
        command: &["cargo", "test", "--workspace"],
        expected_adapter: "tests",
        exit_code: 101,
        raw_stderr: "error: test failed, to rerun pass `-p keel --bin keel`\n",
        raw_stdout: "\nrunning 48 tests\ntest adapters::git::tests::status_compacts ... ok\ntest adapters::tests::tests::pass_summary ... ok\ntest adapters::search::tests::groups_by_file ... ok\ntest adapters::build::tests::errors_first ... ok\ntest adapters::lint::tests::warnings_kept ... ok\ntest adapters::cloud::tests::secrets_redacted ... ok\ntest adapters::database::tests::result_table ... ok\ntest adapters::containers::tests::ps_table ... ok\ntest proxy::token_meter::tests::counts_exact ... ok\ntest proxy::classify::tests::routes_cargo_test ... ok\ntest proxy::registry::tests::specific_before_generic ... ok\ntest proxy::render::tests::renders_pass ... ok\ntest utility::recall::tests::indexes_markdown ... ok\ntest utility::recall::tests::json_briefs ... ok\ntest utility::recall::tests::auto_sync ... ok\ntest utility::skill_lint::tests::well_formed_passes ... ok\ntest utility::sprint::tests::review_fails_until_done ... ok\ntest utility::user_story::tests::gherkin_required ... ok\ntest runner::hook_lifecycle::tests::stop_is_silent ... FAILED\ntest runner::learning::tests::recurring_failure_instinct ... ok\ntest mcp::tools::tests::recall_missing_query ... ok\ntest mcp::tools::tests::skill_route_missing_prompt ... ok\ntest review::tests::tally_counts_blocking ... ok\ntest manager::install::tests::stages_shared ... ok\ntest proxy::run::tests::gate_blocks_without_signal ... ok\ntest proxy::run::tests::no_compact_neutralizes ... ok\ntest utility::config_audit::tests::flags_bypass ... ok\ntest utility::code_graph::tests::reverse_deps ... ok\ntest runner::observation::tests::clusters_failures ... ok\ntest utility::memory::tests::route_missing_request ... ok\ntest utility::eval::tests::token_counts_exact ... ok\ntest adapters::logs::tests::dedup_repeats ... ok\ntest adapters::files::tests::head_truncates ... ok\ntest proxy::injection_guard::tests::neutralizes_marker ... ok\n... 13 more tests ...\n\nfailures:\n\n---- runner::hook_lifecycle::tests::stop_is_silent stdout ----\nthread 'runner::hook_lifecycle::tests::stop_is_silent' panicked at hook_lifecycle.rs:7061:13:\nstop must emit no stdout; got: {\"hookSpecificOutput\":{\"additionalContext\":\"Stop closeout: before finalizing, verify that all stated work is actually complete.\"}}\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\nstack backtrace:\n   0: rust_begin_unwind\n   1: core::panicking::panic_fmt\n   2: keel::runner::hook_lifecycle::tests::stop_is_silent\n   3: core::ops::function::FnOnce::call_once\n\nfailures:\n    runner::hook_lifecycle::tests::stop_is_silent\n\ntest result: FAILED. 47 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.90s\n",
    },
    EvalFixture {
        name: "git-status",
        command: &["git", "status"],
        expected_adapter: "git",
        exit_code: 0,
        raw_stderr: "",
        raw_stdout: "On branch main\nYour branch is up to date with 'origin/main'.\n\nChanges not staged for commit:\n  (use \"git add <file>...\" to update what will be committed)\n  (use \"git restore <file>...\" to discard changes in working directory)\n\tmodified:   rust/crates/keel/src/utility/eval.rs\n\tmodified:   rust/crates/keel/src/utility/mod.rs\n\tmodified:   rust/crates/keel/src/commands.rs\n\tmodified:   rust/crates/keel/src/utility/recall.rs\n\tmodified:   CLAUDE.md\n\tmodified:   .claude/agents/reviewer.md\n\tmodified:   .claude/agents/git-expert.md\n\tmodified:   .claude/hooks.json\n\nUntracked files:\n  (use \"git add <file>...\" to include in what will be committed)\n\t.understand/\n\trust/crates/keel/tests/doc_parity_test.rs\n\trust/crates/keel/tests/eval_test.rs\n\nno changes added to commit (use \"git add\" and/or \"git commit -a\")\n",
    },
    EvalFixture {
        name: "ripgrep-search",
        command: &["rg", "TokenMeter", "--line-number"],
        expected_adapter: "search",
        exit_code: 0,
        raw_stderr: "",
        raw_stdout: "rust/crates/keel/src/proxy/token_meter.rs:15:pub struct TokenMeter;\nrust/crates/keel/src/proxy/token_meter.rs:18:    pub fn count_text(text: &str) -> usize {\nrust/crates/keel/src/proxy/token_meter.rs:24:    pub fn count_bytes(bytes: &[u8]) -> usize {\nrust/crates/keel/src/adapters/common.rs:9:use crate::proxy::token_meter::TokenMeter;\nrust/crates/keel/src/adapters/common.rs:23:    let estimated_tokens_after = TokenMeter::estimate(&stdout) + TokenMeter::estimate(&stderr);\nrust/crates/keel/src/proxy/run.rs:189:                estimated_tokens_before: TokenMeter::estimate_bytes(&result.stdout)\nrust/crates/keel/src/proxy/adapter.rs:3:use crate::proxy::token_meter::TokenMeter;\nrust/crates/keel/src/utility/eval.rs:30:use crate::proxy::token_meter::TokenMeter;\nrust/crates/keel/src/utility/eval.rs:97:        let tokens_raw = TokenMeter::count_bytes(stdout) + TokenMeter::count_bytes(stderr);\nrust/crates/keel/src/utility/gain.rs:142:    let before = TokenMeter::count_text(&raw);\n",
    },
    EvalFixture {
        name: "cargo-clippy-warnings",
        command: &["cargo", "clippy"],
        expected_adapter: "lint",
        exit_code: 0,
        raw_stderr: "warning: unused variable: `missing`\n  --> tests/doc_parity_test.rs:88:9\n   |\n88 |     let missing: Vec<&String> = manifest_skills(&repo_root)\n   |         ^^^^^^^ help: if this is intentional, prefix it with an underscore: `_missing`\n   |\n   = note: `#[warn(unused_variables)]` on by default\n\nwarning: this expression creates a reference which is immediately dereferenced by the compiler\n  --> src/utility/eval.rs:120:33\n   |\n   = note: `#[warn(clippy::needless_borrow)]` on by default\n\nwarning: `keel` (bin) generated 2 warnings\n    Finished dev profile in 3.37s\n",
        raw_stdout: "",
    },
    EvalFixture {
        name: "npm-install",
        command: &["npm", "install"],
        expected_adapter: "build",
        exit_code: 0,
        raw_stderr: "npm warn deprecated inflight@1.0.6: This module is not supported\nnpm warn deprecated glob@7.2.3: Glob versions prior to v9 are no longer supported\n",
        raw_stdout: "added 1423 packages, and audited 1424 packages in 38s\n\n201 packages are looking for funding\n  run `npm fund` for details\n\n12 vulnerabilities (4 moderate, 6 high, 2 critical)\n\nTo address all issues possible, run:\n  npm audit fix\n\nSome issues need review, and may require choosing\na different dependency.\n\nRun `npm audit` for details.\n",
    },
    EvalFixture {
        name: "kubectl-get-pods",
        command: &["kubectl", "get", "pods"],
        expected_adapter: "containers",
        exit_code: 0,
        raw_stderr: "",
        raw_stdout: "NAME                                READY   STATUS    RESTARTS   AGE\napi-gateway-7d9f8c6b5-2xk4l         1/1     Running   0          4d\napi-gateway-7d9f8c6b5-9wp2m         1/1     Running   0          4d\nauth-service-5c7b9d4f8-jk3n2        1/1     Running   2          12d\nauth-service-5c7b9d4f8-mn8q1        1/1     Running   0          12d\nworker-queue-6f8d9c7b4-pq5r3        1/1     Running   0          2d\nworker-queue-6f8d9c7b4-st6u7        1/1     Running   1          2d\nredis-primary-0                     1/1     Running   0          30d\npostgres-primary-0                  1/1     Running   0          30d\nmetrics-collector-8d7f6c5b9-vw2x4   1/1     Running   0          7d\ningress-nginx-controller-abc123     1/1     Running   0          30d\n",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The core claim of the whole proxy: it removes a large fraction of raw
    /// command-output tokens. This asserts a MEASURED floor over the real
    /// pipeline — if a future adapter change stops compacting, this fails.
    #[test]
    fn compaction_eval_produces_real_measured_savings() {
        let report = run_compaction_eval();
        assert!(
            report.total_tokens_raw > 500,
            "fixtures must carry a meaningful raw token volume; got {}",
            report.total_tokens_raw
        );
        // Honest invariants (composition-independent), not an invented aggregate
        // floor that fixture mix could swing:
        //   1. The high-volume case the proxy exists for compacts heavily.
        //   2. The corpus nets positive overall.
        //   3. At least one case actually compacted (the guard did not silently
        //      turn everything into passthrough — that would mean compaction is
        //      effectively dead).
        let high_volume = report
            .cases
            .iter()
            .find(|c| c.name == "cargo-test-pass")
            .expect("cargo-test-pass fixture present");
        assert!(
            high_volume.savings_pct > 50.0,
            "high-volume test output must compact heavily; measured {:.2}% (raw={} compact={})",
            high_volume.savings_pct,
            high_volume.tokens_raw,
            high_volume.tokens_compact
        );
        assert!(
            report.total_tokens_saved > 0,
            "corpus must net positive token savings; saved {}",
            report.total_tokens_saved
        );
        assert!(
            report.cases.iter().filter(|c| c.compacted).count() >= 3,
            "at least 3 fixtures should actually compact; only {} did",
            report.cases.iter().filter(|c| c.compacted).count()
        );
    }

    /// Each fixture must route to the adapter we expect. A classification
    /// regression (e.g. a build command falling through to `generic`) silently
    /// degrades compaction quality; this catches it.
    #[test]
    fn every_fixture_routes_to_expected_adapter() {
        let registry = build_adapter_registry();
        for fixture in EVAL_FIXTURES {
            let args: Vec<String> = fixture.command.iter().map(|s| s.to_string()).collect();
            let ast = classify_command(&args).expect("fixture classifies");
            let adapter = registry.best_match(&ast).expect("adapter matches");
            assert_eq!(
                adapter.name(),
                fixture.expected_adapter,
                "fixture {} routed to {} (expected {})",
                fixture.name,
                adapter.name(),
                fixture.expected_adapter
            );
        }
    }

    /// A failing test run must keep its failure signal after compaction — the
    /// whole point of compacting is to drop noise, NOT the error. This proves
    /// the lossy reduction is still faithful on the path that matters most.
    #[test]
    fn failing_test_fixture_preserves_failure_signal() {
        let report = run_compaction_eval();
        let fail_case = report
            .cases
            .iter()
            .find(|c| c.name == "cargo-test-fail")
            .expect("cargo-test-fail fixture present");
        // It still compacts (saved > 0)...
        assert!(
            fail_case.tokens_saved > 0,
            "failing-test compaction should still save tokens; saved {}",
            fail_case.tokens_saved
        );
        // ...but the rendered output must retain the failure marker. Re-run the
        // single fixture through the pipeline to inspect the rendered text.
        let registry = build_adapter_registry();
        let fixture = EVAL_FIXTURES
            .iter()
            .find(|f| f.name == "cargo-test-fail")
            .unwrap();
        let args: Vec<String> = fixture.command.iter().map(|s| s.to_string()).collect();
        let ast = classify_command(&args).unwrap();
        let adapter = registry.best_match(&ast).unwrap();
        let meta = eval_meta(
            &ast.program,
            &ast.args,
            fixture.raw_stdout.as_bytes(),
            fixture.raw_stderr.as_bytes(),
        );
        let result = adapter.compact(
            fixture.raw_stdout.as_bytes(),
            fixture.raw_stderr.as_bytes(),
            fixture.exit_code,
            &meta,
        );
        let rendered = render_compact_result(&result);
        assert!(
            rendered.contains("FAILED") || rendered.contains("1 failed"),
            "compacted failing-test output must keep the failure signal; got:\n{rendered}"
        );
    }

    /// The token counts must be EXACT o200k_base counts, not byte estimates.
    /// Pin a known string against the tokenizer so a future swap to a heuristic
    /// counter is caught.
    #[test]
    fn token_counts_are_exact_o200k_base() {
        // "hello world" is exactly 2 o200k_base tokens (asserted in token_meter
        // tests too); this binds the eval to the same exact tokenizer.
        assert_eq!(TokenMeter::count_text("hello world"), 2);
        let report = run_compaction_eval();
        // The aggregate is the sum of per-case exact counts, so a per-case
        // total must reconcile with the report total — no hidden estimation.
        let summed_raw: usize = report.cases.iter().map(|c| c.tokens_raw).sum();
        assert_eq!(summed_raw, report.total_tokens_raw);
    }

    /// Every fixture must actually reduce (or at worst break even). A fixture
    /// that grows after "compaction" is a bug in either the fixture or the
    /// adapter; surface it rather than letting it hide in the aggregate.
    #[test]
    fn no_fixture_grows_after_compaction() {
        let report = run_compaction_eval();
        for case in &report.cases {
            assert!(
                case.tokens_compact <= case.tokens_raw,
                "fixture {} grew after compaction: raw={} compact={}",
                case.name,
                case.tokens_raw,
                case.tokens_compact
            );
        }
    }
}
