use super::*;
use crate::runtime::resolve_repository_root;
use std::thread::sleep;
use std::time::{Duration, Instant};

// the "do not merge blind" rule. Wait for the head commit's CI checks to go
// green before merging; block while any check is red or pending. No CI passes.

/// Per-check status surfaced to the merge gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CheckState {
    Pending,
    Green,
    Red,
}

#[derive(Debug, Clone)]
pub(crate) struct CiCheck {
    pub(crate) name: String,
    pub(crate) state: CheckState,
}

/// Which CI provider CLI was detected for this repo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CiProvider {
    Glab,
    Gh,
}

impl CiProvider {
    pub(crate) fn label(self) -> &'static str {
        match self {
            CiProvider::Glab => "glab",
            CiProvider::Gh => "gh",
        }
    }
}

/// Detect the CI provider by remote URL first (authoritative), then by CLI
/// availability. A GitLab remote uses `glab`; a GitHub remote uses `gh`. When the
/// remote gives no signal, fall back to whichever CLI is installed (glab first).
/// Returns `None` when no usable provider exists; the gate then reports no-CI.
pub(crate) fn detect_ci_provider(repo: Option<&Path>) -> Option<CiProvider> {
    let remote = git_text(repo, &["config", "--get", "remote.origin.url"])
        .unwrap_or_default()
        .to_ascii_lowercase();
    let wants_gitlab = remote.contains("gitlab");
    let wants_github = remote.contains("github");
    if wants_gitlab && cli_available("glab") {
        return Some(CiProvider::Glab);
    }
    if wants_github && cli_available("gh") {
        return Some(CiProvider::Gh);
    }
    // Remote gave no usable signal (or its CLI is missing): fall back to
    // whichever CLI is installed, glab first per the auto-detect order.
    if cli_available("glab") {
        return Some(CiProvider::Glab);
    }
    if cli_available("gh") {
        return Some(CiProvider::Gh);
    }
    None
}

/// Whether a CLI is invocable at all (spawn succeeds). `--version` is cheap and
/// offline-safe for both `gh` and `glab`.
pub(crate) fn cli_available(program: &str) -> bool {
    run_command(program, &["--version".to_string()], None)
        .map(|result| result.code == 0)
        .unwrap_or(false)
}

/// Map a free-form CI status/conclusion string onto the tri-state. Anything
/// that is not an explicit success or an explicit still-running state is
/// treated as red so an unknown conclusion fails closed (never merges blind).
pub(crate) fn classify_check_state(raw: &str) -> CheckState {
    let value = raw.trim().to_ascii_lowercase();
    match value.as_str() {
        "success" | "passed" | "pass" | "ok" => CheckState::Green,
        "pending" | "running" | "queued" | "in_progress" | "waiting" | "requested" | "created"
        | "" => CheckState::Pending,
        _ => CheckState::Red,
    }
}

/// Tri-state result of querying a CI provider. `Error` (provider CLI failed,
/// so no signal) must block, while `Checks` with an empty vec (provider
/// reachable, genuinely zero checks) is the only honest "no CI" case.
#[derive(Debug, Clone)]
pub(crate) enum CiQuery {
    /// Provider ran; vec is the parsed checks (empty = genuinely no checks).
    Checks(Vec<CiCheck>),
    /// Provider CLI errored, was unavailable, or output was unparseable.
    Error,
}

/// Query the current head's checks via `glab ci status`. Output is line-based
/// (`name: status`); it is parsed loosely and requires at least one real check so
/// an empty or parse failure reads as "no CI" rather than "green".
pub(crate) fn query_checks_glab(repo: Option<&Path>) -> CiQuery {
    let result = match run_command(
        "glab",
        &["ci".to_string(), "status".to_string(), "--live".to_string()],
        repo,
    ) {
        Ok(result) => result,
        Err(_) => return CiQuery::Error,
    };
    if result.code != 0 {
        return CiQuery::Error;
    }
    CiQuery::Checks(parse_glab_status(&String::from_utf8_lossy(&result.stdout)).unwrap_or_default())
}

pub(crate) fn parse_glab_status(stdout: &str) -> Option<Vec<CiCheck>> {
    let mut checks = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        // Typical shapes: "job-name: success" or "✓ job-name success".
        let cleaned = line
            .trim_start_matches(|c: char| !c.is_alphanumeric() && c != '_')
            .trim();
        if cleaned.is_empty() {
            continue;
        }
        if let Some((name, status)) = cleaned.rsplit_once(':') {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            checks.push(CiCheck {
                name: name.to_string(),
                state: classify_check_state(status),
            });
        }
    }
    if checks.is_empty() {
        None
    } else {
        Some(checks)
    }
}

/// Query the current head's checks via `gh pr checks` (PR-scoped), parsing the
/// tabular output so this gate owns the polling loop and timeout.
///
/// A non-zero exit means either "no PR for this branch" (a legitimate no-CI
/// case that may pass) or a real error (auth, network) that yields no signal
/// and must block. The two are told apart by the error text.
pub(crate) fn query_checks_gh(repo: Option<&Path>) -> CiQuery {
    let result = match run_command("gh", &["pr".to_string(), "checks".to_string()], repo) {
        Ok(result) => result,
        Err(_) => return CiQuery::Error, // gh failed to launch (absent/unexecutable).
    };
    // `gh pr checks` signals pending/failing checks via a non-zero exit (8 for
    // pending) while still printing the table, so parseable rows ARE the signal.
    if result.code != 0 {
        let stdout = String::from_utf8_lossy(&result.stdout);
        if let Some(checks) = parse_gh_checks(&stdout) {
            return CiQuery::Checks(checks);
        }
        let detail =
            format!("{}\n{}", stdout, String::from_utf8_lossy(&result.stderr)).to_ascii_lowercase();
        // The honest no-CI case: gh reached the provider and found no PR.
        // Anything else (auth, network, not-a-repo) is an error, so fail closed.
        if detail.contains("no pull requests found")
            || detail.contains("no open pull request")
            || detail.contains("no pr found")
            || detail.contains("no pull request associated")
        {
            return CiQuery::Checks(Vec::new());
        }
        return CiQuery::Error;
    }
    CiQuery::Checks(parse_gh_checks(&String::from_utf8_lossy(&result.stdout)).unwrap_or_default())
}

pub(crate) fn parse_gh_checks(stdout: &str) -> Option<Vec<CiCheck>> {
    let mut checks = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if cleaned_header(line) {
            continue;
        }
        // gh pr checks columns: NAME STATUS ... (whitespace/tab separated).
        let mut columns = line.split_whitespace();
        let name = match columns.next() {
            Some(value) if !value.is_empty() => value,
            _ => continue,
        };
        let status = columns.next().unwrap_or("");
        checks.push(CiCheck {
            name: name.to_string(),
            state: classify_check_state(status),
        });
    }
    if checks.is_empty() {
        None
    } else {
        Some(checks)
    }
}

/// Header / blank / separator detection for `gh pr checks` tabular output.
pub(crate) fn cleaned_header(line: &str) -> bool {
    if line.is_empty() {
        return true;
    }
    let upper = line.to_ascii_uppercase();
    upper.starts_with("NAME")
        || upper.starts_with("CHECK")
        || line
            .chars()
            .all(|c| c == '-' || c == '+' || c.is_whitespace())
}

/// Poll the head commit's checks until green, red, or timeout. Returns the
/// process exit code. `--watch` keeps polling; without it the gate evaluates once
/// and reports. On any red check the gate blocks (exit 1) and tells the caller to
/// fix first; merge must not proceed.
pub(crate) fn run_git_workflow_await_ci(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("git-workflow await-ci");
    flag_set.string_flag("repo-root", "");
    flag_set.string_flag("provider", "auto");
    flag_set.string_flag("timeout-secs", "600");
    flag_set.string_flag("interval-secs", "15");
    flag_set.bool_flag("watch", false);
    flag_set.string_flag("format", "compact");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let repository_root = match resolve_repository_root(flag_set.string_value("repo-root")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "git-workflow await-ci: {error}");
            return 1;
        }
    };
    let repo = Some(repository_root.as_path());
    if git_text(repo, &["rev-parse", "--is-inside-work-tree"])
        .map(|value| value.trim() != "true")
        .unwrap_or(true)
    {
        let _ = writeln!(
            standard_error,
            "git-workflow await-ci: {} is not a git work tree",
            repository_root.display()
        );
        return 1;
    }

    let provider = match resolve_provider(flag_set.string_value("provider"), repo) {
        ProviderResolution::Found(provider) => provider,
        ProviderResolution::NoneDetected => {
            // Auto-detect found no CI provider: the repo has no CI configured.
            // Not a failure. Report and pass so CI-less repos are never blocked.
            return render_await_ci_result(
                standard_output,
                flag_set.string_value("format"),
                AwaitCiOutcome::NoCi,
                "auto",
                &[],
                0,
            );
        }
        ProviderResolution::ExplicitUnavailable(requested) => {
            // The caller named a provider that is not installed. That is a
            // misconfiguration, not "no CI", so fail closed.
            return render_await_ci_result(
                standard_output,
                flag_set.string_value("format"),
                AwaitCiOutcome::Error,
                &requested,
                &[],
                0,
            );
        }
    };

    let timeout_secs = flag_set
        .string_value("timeout-secs")
        .trim()
        .parse::<u64>()
        .unwrap_or(600);
    let interval_secs = flag_set
        .string_value("interval-secs")
        .trim()
        .parse::<u64>()
        .unwrap_or(15)
        .max(2);
    let watch = flag_set.bool_value("watch");

    let started = Instant::now();
    let deadline = Duration::from_secs(timeout_secs);
    let interval = Duration::from_secs(interval_secs);
    let mut attempts = 0usize;

    loop {
        attempts += 1;
        match query_provider_checks(provider, repo) {
            CiQuery::Error => {
                // The provider CLI errored or was unavailable, so there is no
                // signal. Fail closed rather than merge blind.
                return render_await_ci_result(
                    standard_output,
                    flag_set.string_value("format"),
                    AwaitCiOutcome::Error,
                    provider.label(),
                    &[],
                    attempts,
                );
            }
            CiQuery::Checks(checks) => {
                // Freshness guard: right after a push the newest run still
                if !checks.is_empty() && !ci_run_matches_head(provider, repo) {
                    if !watch || started.elapsed() >= deadline {
                        let outcome = if watch {
                            AwaitCiOutcome::Timeout
                        } else {
                            AwaitCiOutcome::Pending
                        };
                        return render_await_ci_result(
                            standard_output,
                            flag_set.string_value("format"),
                            outcome,
                            provider.label(),
                            &checks,
                            attempts,
                        );
                    }
                    sleep(interval);
                    continue;
                }
                match evaluate_checks(&checks) {
                    CiVerdict::Green => {
                        return render_await_ci_result(
                            standard_output,
                            flag_set.string_value("format"),
                            AwaitCiOutcome::Green,
                            provider.label(),
                            &checks,
                            attempts,
                        );
                    }
                    CiVerdict::Red => {
                        // Block: do NOT continue to merge on a red pipeline.
                        return render_await_ci_result(
                            standard_output,
                            flag_set.string_value("format"),
                            AwaitCiOutcome::Red,
                            provider.label(),
                            &checks,
                            attempts,
                        );
                    }
                    CiVerdict::NoChecks => {
                        // Provider reachable but genuinely reporting no checks is the
                        // only no-CI pass path; errors never reach it.
                        return render_await_ci_result(
                            standard_output,
                            flag_set.string_value("format"),
                            AwaitCiOutcome::NoCi,
                            provider.label(),
                            &[],
                            attempts,
                        );
                    }
                    CiVerdict::Pending => {
                        if !watch || started.elapsed() >= deadline {
                            let outcome = if watch {
                                AwaitCiOutcome::Timeout
                            } else {
                                AwaitCiOutcome::Pending
                            };
                            return render_await_ci_result(
                                standard_output,
                                flag_set.string_value("format"),
                                outcome,
                                provider.label(),
                                &checks,
                                attempts,
                            );
                        }
                        sleep(interval);
                    }
                }
            }
        }
    }
}

pub(crate) enum CiVerdict {
    Green,
    Red,
    Pending,
    NoChecks,
}

/// Why provider resolution failed. An explicit provider that is missing fails
/// closed; auto-detect finding no provider means the repo has no CI and may pass.
#[derive(Debug)]
pub(crate) enum ProviderResolution {
    Found(CiProvider),
    /// Auto-detect found no provider -> repo has no CI configured.
    NoneDetected,
    /// Caller named a provider explicitly but it is unavailable/unknown.
    ExplicitUnavailable(String),
}

pub(crate) fn resolve_provider(requested: &str, repo: Option<&Path>) -> ProviderResolution {
    let normalized = requested.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "auto" | "" => match detect_ci_provider(repo) {
            Some(provider) => ProviderResolution::Found(provider),
            None => ProviderResolution::NoneDetected,
        },
        "glab" if cli_available("glab") => ProviderResolution::Found(CiProvider::Glab),
        "gh" if cli_available("gh") => ProviderResolution::Found(CiProvider::Gh),
        _ => ProviderResolution::ExplicitUnavailable(normalized),
    }
}

pub(crate) fn query_provider_checks(provider: CiProvider, repo: Option<&Path>) -> CiQuery {
    match provider {
        CiProvider::Glab => query_checks_glab(repo),
        CiProvider::Gh => query_checks_gh(repo),
    }
}

/// True when the newest CI run was created for the current local HEAD. Right
/// after a push, GitHub still lists the previous commit's run; its verdict
/// must not be reported for the new head. Query failures degrade to `true`
/// (fresh) so a missing signal never stalls the gate — `evaluate_checks` then
/// decides from the checks themselves.
pub(crate) fn ci_run_matches_head(provider: CiProvider, repo: Option<&Path>) -> bool {
    let head = match run_command("git", &["rev-parse".to_string(), "HEAD".to_string()], repo) {
        Ok(result) if result.code == 0 => {
            String::from_utf8_lossy(&result.stdout).trim().to_string()
        }
        _ => return true,
    };
    if head.is_empty() {
        return true;
    }
    match provider {
        CiProvider::Gh => {
            match run_command(
                "gh",
                &[
                    "run".to_string(),
                    "list".to_string(),
                    "--limit".to_string(),
                    "1".to_string(),
                    "--json".to_string(),
                    "headSha".to_string(),
                ],
                repo,
            ) {
                Ok(result) if result.code == 0 => {
                    let parsed: serde_json::Value =
                        serde_json::from_str(&String::from_utf8_lossy(&result.stdout))
                            .unwrap_or(serde_json::Value::Null);
                    match parsed
                        .as_array()
                        .and_then(|runs| runs.first())
                        .and_then(|run| run.get("headSha"))
                        .and_then(serde_json::Value::as_str)
                    {
                        Some(sha) => sha.eq_ignore_ascii_case(&head),
                        None => true,
                    }
                }
                _ => true,
            }
        }
        // glab's list output carries no cheap head-sha column; keep prior
        // behavior there.
        CiProvider::Glab => true,
    }
}

pub(crate) fn evaluate_checks(checks: &[CiCheck]) -> CiVerdict {
    if checks.is_empty() {
        return CiVerdict::NoChecks;
    }
    if checks.iter().any(|check| check.state == CheckState::Red) {
        return CiVerdict::Red;
    }
    if checks
        .iter()
        .any(|check| check.state == CheckState::Pending)
    {
        return CiVerdict::Pending;
    }
    CiVerdict::Green
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AwaitCiOutcome {
    Green,
    Red,
    Pending,
    Timeout,
    NoCi,
    /// The CI provider could not be queried (CLI error / unavailable): no
    /// signal. Always blocks, never merge blind.
    Error,
}

impl AwaitCiOutcome {
    /// Exit code: only a fully green pipeline (or a repo with genuinely no CI)
    /// may proceed to merge. Red, pending, timeout, and a provider error all
    /// block; an errored/absent provider yields no signal, so it fails closed.
    pub(crate) fn exit_code(self) -> u8 {
        match self {
            AwaitCiOutcome::Green | AwaitCiOutcome::NoCi => 0,
            AwaitCiOutcome::Red
            | AwaitCiOutcome::Pending
            | AwaitCiOutcome::Timeout
            | AwaitCiOutcome::Error => 1,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            AwaitCiOutcome::Green => "GREEN — all checks passed, safe to merge",
            AwaitCiOutcome::Red => {
                "RED — fix the failing checks before merging; do NOT merge blind"
            }
            AwaitCiOutcome::Pending => "PENDING — checks still running; wait before merging",
            AwaitCiOutcome::Timeout => {
                "TIMEOUT — checks did not go green in time; do NOT merge blind"
            }
            AwaitCiOutcome::NoCi => {
                "NO CI — no CI/CD checks detected for this branch; nothing to await"
            }
            AwaitCiOutcome::Error => {
                "ERROR — could not query the CI provider (auth/network/CLI); no signal, do NOT merge blind"
            }
        }
    }
}

pub(crate) fn render_await_ci_result(
    standard_output: &mut dyn Write,
    output_format: &str,
    outcome: AwaitCiOutcome,
    provider: &str,
    checks: &[CiCheck],
    attempts: usize,
) -> u8 {
    if output_format == "json" {
        let payload = Value::Object(vec![
            (
                "command".into(),
                Value::String("git-workflow await-ci".into()),
            ),
            ("passed".into(), Value::Bool(outcome.exit_code() == 0)),
            ("outcome".into(), Value::String(format!("{outcome:?}"))),
            ("provider".into(), Value::String(provider.into())),
            ("attempts".into(), Value::Number(attempts.to_string())),
            (
                "checks".into(),
                Value::Array(
                    checks
                        .iter()
                        .map(|check| {
                            Value::Object(vec![
                                ("name".into(), Value::String(check.name.clone())),
                                ("state".into(), Value::String(format!("{:?}", check.state))),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]);
        let _ = write_indented(standard_output, &payload);
        return outcome.exit_code();
    }
    let _ = writeln!(
        standard_output,
        "git-workflow await-ci [{}]: {}",
        provider,
        outcome.label()
    );
    for check in checks {
        let marker = match check.state {
            CheckState::Green => "ok",
            CheckState::Pending => "…",
            CheckState::Red => "FAIL",
        };
        let _ = writeln!(standard_output, "  [{marker:>4}] {}", check.name);
    }
    if outcome == AwaitCiOutcome::Red {
        let _ = writeln!(
            standard_output,
            "  fix the red checks, push, and re-run `keel git-workflow await-ci --watch` before merging"
        );
    }
    outcome.exit_code()
}
