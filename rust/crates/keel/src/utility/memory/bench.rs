//! Purpose: Bench command (runtime feature-parity marker, not a measurement)
//! Caller: mod.rs run_memory_command
//! Dependencies: std::io::Write, crate::args::FlagSet, crate::json
//! Main Functions: run_bench_command
//! Side Effects: None

use std::io::Write;

use crate::args::FlagSet;
use crate::json::Value;

pub(super) fn run_bench_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("bench");
    flag_set.bool_flag("json", false);
    flag_set.bool_flag("fixtures", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let fixtures = benchmark_fixtures();
    let raw_bytes: usize = fixtures.iter().map(|fixture| fixture.raw_bytes).sum();
    let compacted_bytes: usize = fixtures.iter().map(|fixture| fixture.compacted_bytes).sum();
    let saved_bytes = raw_bytes.saturating_sub(compacted_bytes);
    let savings_percent = if raw_bytes == 0 {
        0.0
    } else {
        (saved_bytes as f64 / raw_bytes as f64) * 100.0
    };
    if flag_set.bool_value("json") {
        let payload = Value::Object(vec![
            ("runtime".into(), Value::String("rust".into())),
            ("goFallback".into(), Value::Bool(false)),
            (
                "thirdPartyRuntimeDependencies".into(),
                Value::Array(Vec::new()),
            ),
            (
                "benchmarkRole".into(),
                Value::String("feature-parity".into()),
            ),
            (
                "fixtureCount".into(),
                Value::Number(fixtures.len().to_string()),
            ),
            ("rawBytes".into(), Value::Number(raw_bytes.to_string())),
            (
                "compactedBytes".into(),
                Value::Number(compacted_bytes.to_string()),
            ),
            ("savedBytes".into(), Value::Number(saved_bytes.to_string())),
            (
                "savingsPercent".into(),
                Value::Number(format!("{savings_percent:.2}")),
            ),
            (
                "features".into(),
                Value::Array(
                    [
                        "shell-aware rewrite",
                        "command-specific semantic reducers",
                        "bounded streaming",
                        "raw-output recovery",
                        "persisted gain analytics",
                        "the harness lifecycle hook integration",
                    ]
                    .iter()
                    .map(|feature| Value::String((*feature).into()))
                    .collect(),
                ),
            ),
        ]);
        return crate::json::write_indented(standard_output, &payload).map_or(1, |_| 0);
    }
    let _ = writeln!(
        standard_output,
        "keel bench: rust-native runtime + feature-parity marker (not a measurement)"
    );
    let _ = writeln!(
        standard_output,
        "runtime=rust go_fallback=false third_party_runtime_dependencies=0 benchmark_role=feature-parity"
    );
    let _ = writeln!(
        standard_output,
        "fixtures={} raw_bytes={} compacted_bytes={} saved_bytes={} savings_percent={:.2} (illustrative fixtures — run `keel eval` for real measured token savings)",
        fixtures.len(),
        raw_bytes,
        compacted_bytes,
        saved_bytes,
        savings_percent
    );
    if flag_set.bool_value("fixtures") {
        for fixture in fixtures {
            let _ = writeln!(
                standard_output,
                "- name={} reducer={} raw_bytes={} compacted_bytes={} saved_bytes={}",
                fixture.name,
                fixture.reducer,
                fixture.raw_bytes,
                fixture.compacted_bytes,
                fixture.raw_bytes.saturating_sub(fixture.compacted_bytes)
            );
        }
    }
    0
}

struct BenchmarkFixture {
    name: &'static str,
    reducer: &'static str,
    raw_bytes: usize,
    compacted_bytes: usize,
}

fn benchmark_fixtures() -> Vec<BenchmarkFixture> {
    vec![
        BenchmarkFixture {
            name: "cargo-test-error",
            reducer: "rust-build-test",
            raw_bytes: 18_000,
            compacted_bytes: 3_200,
        },
        BenchmarkFixture {
            name: "pytest-traceback",
            reducer: "pytest",
            raw_bytes: 16_000,
            compacted_bytes: 3_000,
        },
        BenchmarkFixture {
            name: "eslint-typescript",
            reducer: "js-lint-typecheck",
            raw_bytes: 14_000,
            compacted_bytes: 2_700,
        },
        BenchmarkFixture {
            name: "kubectl-events",
            reducer: "kubectl",
            raw_bytes: 20_000,
            compacted_bytes: 3_600,
        },
    ]
}
