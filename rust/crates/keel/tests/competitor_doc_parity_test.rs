//! Competitor-claim doc parity.
//!
//! Purpose: keep competitive docs from reintroducing stale peer-tool claims after
//! a web re-verify. Scratch notes alone do not gate CI; this test scans the
//! checked-in docs for forbidden outdated substrings.
//!
//! Verified fact (web research 2026-07, RTK GitHub README): since RTK v0.37.2 the
//! auto-rewrite hook is a native binary (`rtk hook claude` / `rtk init -g`) on
//! Command Prompt, PowerShell, and Windows Terminal. Claims that RTK has
//! "no auto-rewrite on native Windows" are false and must not reappear.

use std::fs;
use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("workspace repository root")
        .to_path_buf()
}

/// Substrings that must not appear in competitive docs after the 2026-07 RTK re-verify.
/// Each entry is a stale *current* claim. Historical notes may say "pre-v0.37.2"
/// without using these residual scorecard phrases.
const FORBIDDEN_STALE_RTK_WINDOWS_CLAIMS: &[&str] = &[
    // Original audit denials
    "no native-Windows auto-rewrite",
    "no auto-rewrite at all on native Windows",
    "no auto-rewrite on native Windows",
    "No auto-rewrite on native Windows",
    "Windows auto-rewrite: NOT supported",
    // Residual scorecard / tier wording that still denied current Windows support
    "POSIX only",
    "hook, POSIX only",
    "Windows fallback to CLAUDE.md",
    "fallback to CLAUDE.md = Tier C",
];

/// Docs that name RTK/superpowers/ECC and carry competitive capability claims.
fn competitive_doc_paths(repo_root: &Path) -> Vec<PathBuf> {
    [
        "docs/competitive-gap-closure.md",
        "KEEL-AUDIT-2026-06.md",
        "docs/benchmark-comparison-scorecard.md",
        "docs/audits/2026-06-12-harness-competitor-gap-audit/findings.md",
    ]
    .iter()
    .map(|rel| repo_root.join(rel))
    .filter(|p| p.is_file())
    .collect()
}

#[test]
fn competitive_docs_forbid_stale_rtk_windows_rewrite_claims() {
    let repo_root = repository_root();
    let docs = competitive_doc_paths(&repo_root);
    assert!(
        !docs.is_empty(),
        "expected at least docs/competitive-gap-closure.md under {}",
        repo_root.display()
    );

    let mut hits: Vec<String> = Vec::new();
    for path in &docs {
        let text = fs::read_to_string(path).unwrap_or_else(|e| {
            panic!("read {}: {e}", path.display());
        });
        for needle in FORBIDDEN_STALE_RTK_WINDOWS_CLAIMS {
            if text.contains(needle) {
                // Line numbers for fix guidance
                for (idx, line) in text.lines().enumerate() {
                    if line.contains(needle) {
                        hits.push(format!(
                            "{}:{} contains forbidden stale RTK claim: {:?}",
                            path.strip_prefix(&repo_root).unwrap_or(path).display(),
                            idx + 1,
                            needle
                        ));
                    }
                }
            }
        }
    }

    assert!(
        hits.is_empty(),
        "stale RTK Windows rewrite claims found (web re-verify 2026-07: RTK v0.37.2+ has native Windows auto-rewrite via `rtk hook claude`). Fix wording, then re-run this test.\n{}",
        hits.join("\n")
    );
}

#[test]
fn competitive_gap_closure_states_rtk_windows_native_hook() {
    // Positive guard: after removing stale claims, the primary competitive doc
    // must still state the current truth so accuracy is not "delete only".
    let path = repository_root().join("docs/competitive-gap-closure.md");
    let text = fs::read_to_string(&path).expect("read competitive-gap-closure.md");
    assert!(
        text.contains("v0.37.2") && text.to_ascii_lowercase().contains("native"),
        "docs/competitive-gap-closure.md must document RTK v0.37.2+ native Windows rewrite (not only delete stale claims)"
    );
    assert!(
        text.contains("rtk hook claude") || text.contains("rtk init"),
        "docs/competitive-gap-closure.md should name the RTK native hook surface (`rtk hook claude` or `rtk init`)"
    );
}
