---
name: adversarial-security-review
description: Stress-test code and configuration from an attacker's perspective using a structured red-team / blue-team / adjudicator pass, beyond a checklist scan. Use when a change touches auth, secrets, input handling, agent/hook config, permissions, or anything an attacker would target — first think like the attacker (enumerate concrete exploit paths), then like the defender (existing mitigations), then adjudicate each claimed finding to a confirmed/refuted verdict with evidence so false positives do not drown the real risk. Use when the user says "security review", "threat model this", "can this be exploited", or before shipping security-sensitive code. Complements claude-skills config-audit (the deterministic static scan) and security-and-compliance-auditor (the standards/compliance lens).
when_to_use: Security-sensitive changes — auth, secrets, input validation, agent/hook/MCP config, permissions, data handling. Run the red-team/blue-team/adjudicator loop to confirm exploitability with evidence. Complements config-audit (static scan) and security-and-compliance-auditor (compliance).
allowed-tools: Read, Grep, Glob, Bash(claude-skills config-audit:*)
context: fork
agent: general-purpose
model: opus
effort: xhigh
---

# Adversarial Security Review

## Purpose

Find the exploitable security problems a checklist misses, and prove which claimed
problems are actually exploitable. A static scan flags patterns; an attacker
chains them. The failure this prevents is two-sided: shipping a real vulnerability
because no one thought like an attacker, and burning trust on a flood of
theoretical "findings" that are not actually reachable. This skill runs a
structured three-role pass — red team, blue team, adjudicator — so the output is a
short list of *confirmed* risks with evidence, not a wall of maybes. It complements
`claude-skills config-audit` (deterministic static scan of claude-core's own config)
and `security-and-compliance-auditor` (the standards/compliance lens).

## Code Implementation Discipline

See `_shared/common-discipline.md` § Code Implementation Discipline. Adjudication is
**Think Before Coding** applied to risk: a flagged pattern is a hypothesis, not a
finding — confirm it sits on a reachable, attacker-controllable path before calling
it a vulnerability. **No Workarounds** governs the fix: remediate the root cause
(validate at the trust boundary, scope the permission) rather than masking the
symptom.

## The Three Roles

Run them in order. Each is a distinct mode of thinking; do not collapse them, or you
get either paranoia (red with no adjudication) or complacency (blue with no red).

### 1. Red team — enumerate concrete exploit paths

Adopt the attacker's goal and ask how the change helps them. Be specific and
concrete — name the input, the path, and the payoff, not "this could be unsafe":

- **Untrusted input**: where does attacker-controlled data enter, and where does it
  reach a sink (a query, a shell, a template, a deserializer, a file path)? Trace
  injection (SQL/command/template), path traversal, SSRF, and unsafe deserialization.
- **AuthN/AuthZ**: can a step be skipped, replayed, or reached without the right
  identity or role? Look for missing checks, IDOR (object refs with no ownership
  check), and trust placed in client-supplied values.
- **Secrets**: are credentials hardcoded, logged, echoed, committed, or sent to a
  third party? Reference secrets by key name, never echo their values.
- **Agent/hook/MCP config** (the AgentShield surface): hooks that interpolate
  untrusted text into a shell, network-fetching hooks, `bypassPermissions`, unscoped
  `Bash` allow-rules, auto-run agents with unrestricted tools, unpinned `npx`/remote
  MCP servers, prompt-injection surfaces in agent definitions.
- **Supply chain**: unpinned or typosquattable dependencies, postinstall scripts.

For each, write the **attack as a concrete scenario**: attacker controls X → reaches
Y → achieves Z.

### 2. Blue team — map existing mitigations

For each red-team scenario, find what already stops it: input validation at the
boundary, parameterized queries, an auth middleware, a permission scope, output
encoding, a deny-list. State precisely where the mitigation lives (file:line) and
whether it actually covers the traced path — a mitigation on a *different* path does
not count.

### 3. Adjudicator — verdict with evidence

Reconcile red against blue. For each scenario, render one verdict:

- **Confirmed** — the path is reachable and unmitigated. Provide the evidence: the
  entry point, the unprotected sink, and why the existing controls do not cover it.
  Rate severity by realistic impact × reachability.
- **Refuted** — a real, correctly-scoped mitigation blocks it. Name the control.
  Say so plainly so effort is not wasted re-litigating it.
- **Needs proof** — cannot determine from reading alone. State exactly what would
  confirm it (a specific test, a payload, a runtime check). Do not inflate it to
  "confirmed" or dismiss it to "refuted" without that evidence.

Only Confirmed findings are vulnerabilities. This adjudication step is what keeps the
review trustworthy instead of a false-positive flood.

## Pair With The Deterministic Scan

Run `claude-skills config-audit` first when the change touches claude-core's own
hook/settings/manifest surface — it deterministically flags the mechanical issues
(shell-metacharacter injection, network hooks, `bypassPermissions`, unscoped Bash,
committed secret literals) and fails closed on high findings. This skill is the
*reasoning* layer above that scan: it chains findings into real attack paths and
adjudicates exploitability, which a static scan cannot do.

## Remediation

For each Confirmed finding, fix the root cause at the trust boundary, then re-run
the relevant role to confirm the path is now closed:

- Validate/encode untrusted input where it crosses the boundary, not three layers
  downstream.
- Use parameterized queries / safe APIs instead of string-built commands.
- Add the missing authorization check at the resource owner.
- Scope the permission, pin the dependency, sign/verify the webhook, move the secret
  to a referenced env var.

## Anti-Patterns

- Reporting flagged patterns as vulnerabilities without proving the path is reachable
  (false-positive flood that trains people to ignore the report).
- Stopping at the static scan and never thinking like an attacker (misses chained
  exploits the scanner cannot see).
- Red-teaming with no adjudication, so every theoretical concern reads as critical.
- Echoing secret values into the report instead of referencing them by key name.
- "Fixing" by catching the error downstream instead of validating at the boundary.
- Marking a scenario refuted on the basis of a mitigation that guards a different path.

## Validation

Methodology skill; pairs with `claude-skills config-audit`. Self-check before
claiming a security review done: did you enumerate concrete attacker scenarios (red),
map the actual mitigations with file:line (blue), and adjudicate each to
confirmed/refuted/needs-proof with evidence — and does every Confirmed finding name
the reachable path and a root-cause remediation? If findings are listed without an
exploitability verdict, it is a scan, not an adversarial review.
