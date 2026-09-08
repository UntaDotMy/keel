# Agent Quality, Planning, and Verification Research

- Repository: `UntaDotMy/keel`
- Retrieval date: 2026-09-09 (Asia/Singapore)
- Grounding status: live web plus current local code
- Scope: the integrated agent-quality program, Phases 0 through 12

## Conclusions that constrain the program

1. Specification, plan, tasks, and implementation need separate validated artifacts. A prose reminder is not an enforcement boundary.
2. Acceptance examples are useful when they are executable and agreed, but a checklist alone is not proof that the implementation satisfies the requirement.
3. Planning and multi-agent research results are design inputs, not Keel benchmarks. Keel must measure its own outcomes.
4. Current web grounding must retain source metadata. A response without source metadata is ungrounded even when the model produced an answer.
5. Traceability must be bidirectional enough to expose missing decomposition, duplication, and unowned evidence.
6. Screenshot comparison is environment-sensitive and cannot resolve every visual oracle. An unclear verdict must require human review.
7. Static diagnostics are valuable but imperfect. New warnings should block by default while baselines and time-bounded, reasoned waivers preserve operability.
8. Mutation testing measures whether tests detect injected changes, but it requires a stable passing baseline and does not replace functional verification.
9. Deferred tool loading is host-specific. Keel must retain the full-capability path and measure catalog cost and task success before changing defaults.

## Source records

### SRC-001: GitHub Spec Kit

- Title: GitHub Spec Kit documentation
- URL / identifier: https://github.com/github/spec-kit/blob/main/docs/index.md
- Source type: official repository documentation
- Publication date: continuously maintained; page does not state a fixed publication date
- Retrieval date: 2026-09-09
- Original takeaway: Spec Kit exposes a deliberate Spec → Plan → Tasks → Implement sequence in which each phase produces an artifact for the next phase. This supports separate Keel plan artifacts and cross-artifact validation instead of a single unstructured prompt.
- Conditions and limitations: This describes GitHub Spec Kit behavior, not measured quality for Keel. The repository is active, so command and integration details are current only as of retrieval.
- Program phase affected: Phase 2 planner and Phase 9 lifecycle enforcement

### SRC-002: Cucumber Behaviour-Driven Development

- Title: Behaviour-Driven Development
- URL / identifier: https://cucumber.io/docs/bdd/
- Source type: official documentation
- Publication date: continuously maintained; page does not state a fixed publication date
- Retrieval date: 2026-09-09
- Original takeaway: Cucumber frames executable specification as agreed concrete examples, then automation, then implementation guided by those examples. Keel acceptance criteria should therefore include observable preconditions, actions, outcomes, and evidence methods rather than vague quality adjectives.
- Conditions and limitations: BDD examples cover behavior well but do not by themselves prove security, performance, compatibility, or visual quality.
- Program phase affected: Phase 2 acceptance criteria and Phase 5 ticket evidence

### SRC-003: Grounded Iterative Language Planning

- Title: Grounded Iterative Language Planning: How Parameterized World Models Reduce Hallucination Propagation in LLM Agents
- URL / identifier: https://arxiv.org/abs/2606.27806
- Source type: research paper (preprint)
- Publication date: 2026-06-26
- Retrieval date: 2026-09-09
- Original takeaway: The paper evaluates consistency gates between proposed actions and an external state model, supporting Keel's use of deterministic checks that reject unsupported plan transitions instead of trusting a planner's self-report.
- Conditions and limitations: The reported results use graph-structured planning benchmarks and selected model calls. They are not Keel results and do not establish an effect size for software delivery.
- Program phase affected: Phase 2 planner, Phase 3 grounding, and Phase 7 independent review

### SRC-004: ReWOO

- Title: ReWOO: Decoupling Reasoning from Observations for Efficient Augmented Language Models
- URL / identifier: https://arxiv.org/abs/2305.18323
- Source type: research paper (historical)
- Publication date: 2023-05-23
- Retrieval date: 2026-09-09
- Original takeaway: ReWOO separates planning from tool observations to reduce repeated reasoning and execution. Keel can borrow the separation of plan, evidence collection, and execution without copying the paper's architecture or performance claims.
- Conditions and limitations: Historical research with benchmark-specific results. Its reported token and accuracy numbers must never be presented as Keel measurements.
- Program phase affected: Phase 2 planner and Phase 12 benchmark design

### SRC-005: Multi-agent failure taxonomy

- Title: Why Do Multi-Agent LLM Systems Fail?
- URL / identifier: https://arxiv.org/abs/2503.13657
- Source type: research paper
- Publication date: 2025-03-17
- Retrieval date: 2026-09-09
- Original takeaway: The study analyzes failure modes across multiple multi-agent frameworks and tasks, reinforcing the need for shared requirements, explicit hand-off artifacts, and independent evidence checks rather than chained natural-language conclusions.
- Conditions and limitations: The study covers five frameworks and more than 150 tasks, not Keel's hosts or repository workflows. Its taxonomy is a design input, not proof that a Keel gate prevents every propagation failure.
- Program phase affected: Phase 5 tickets, Phase 7 review, and Phase 9 lifecycle enforcement

### SRC-006: Google Search grounding

- Title: Grounding with Google Search
- URL / identifier: https://cloud.google.com/vertex-ai/generative-ai/docs/grounding/grounding-with-google-search
- Source type: official vendor documentation
- Publication date: continuously maintained; page does not state a fixed publication date
- Retrieval date: 2026-09-09
- Original takeaway: Google's current documentation positions search grounding as a route to up-to-date public information and returns source chunks and support mappings when grounding succeeds. It also states that source metadata can be absent, in which case the response was not grounded.
- Conditions and limitations: This contract applies to Google's grounding product, not every host search implementation. Keel must record the selected adapter and treat missing citations as ungrounded.
- Program phase affected: Phase 3 research freshness and Phase 7 grounding gate

### SRC-007: NASA requirements traceability

- Title: Systems Engineering Handbook, MSFC-HDBK-3173 Revision C
- URL / identifier: https://standards.nasa.gov/sites/default/files/standards/MSFC/C/0/msfc-hdbk-3173_rev_c.pdf
- Source type: government handbook (historical standard guidance)
- Publication date: 2018-11-09 effective date
- Retrieval date: 2026-09-09
- Original takeaway: The handbook requires requirements to trace to parent or source requirements, calls for completeness of decomposition, and uses a requirements matrix to reveal missing allocation, duplication, and gold plating. Keel's RTM should reject dangling links in both directions.
- Conditions and limitations: The handbook targets systems engineering and is not a software-specific CLI schema. Keel should adopt the traceability principle without importing process weight that has no observable software-delivery benefit.
- Program phase affected: Phase 5 RTM and Phase 7 plan traceability gate

### SRC-008: Guided and checklist code review

- Title: Do explicit review strategies improve code review performance? Towards understanding the role of cognitive load
- URL / identifier: https://doi.org/10.1007/s10664-022-10123-8
- Source type: peer-reviewed research paper
- Publication date: 2022
- Retrieval date: 2026-09-09
- Original takeaway: The controlled study compares ad hoc review, a checklist, and guided checklist execution. It supports structured review guidance while also distinguishing a checklist of items from a strategy that tells a reviewer how to examine evidence.
- Conditions and limitations: The experiment used 70 developers in one organizational setting and three review tasks. Results may not generalize to automated agent review or this repository.
- Program phase affected: Phase 7 evidence-driven review

### SRC-009: Replicated requirements inspection experiment

- Title: A Replicated Experiment to Assess Requirements Inspection Techniques
- URL / identifier: https://doi.org/10.1023/A:1009742216007
- Source type: peer-reviewed research paper (historical)
- Publication date: 1997
- Retrieval date: 2026-09-09
- Original takeaway: The replication reported results that differed in part from the original experiment and did not find evidence that one scenario technique consistently outperformed alternatives. This is a direct warning against treating checklist presence as proof of review quality.
- Conditions and limitations: Historical study of human requirements inspections in a specific environment. It establishes uncertainty and replication sensitivity, not a current agent-review benchmark.
- Program phase affected: Phase 7 review semantics and Phase 10 quality claims

### SRC-010: Playwright visual comparisons

- Title: Visual comparisons
- URL / identifier: https://playwright.dev/docs/test-snapshots
- Source type: official documentation
- Publication date: continuously maintained; page does not state a fixed publication date
- Retrieval date: 2026-09-09
- Original takeaway: Playwright provides screenshot capture and comparison with explicit difference thresholds, while warning that rendering varies with operating system, browser version, settings, hardware, and execution mode. Keel visual evidence must retain environment and adapter identity.
- Conditions and limitations: Pixel comparison can detect change but cannot determine whether every visible result is semantically correct. Deterministic fixtures and non-visual assertions remain necessary.
- Program phase affected: Phase 8 UI verification

### SRC-011: GUI visual-oracle research

- Title: Extraction and empirical evaluation of GUI-level invariants as GUI Oracles in mobile app testing
- URL / identifier: https://www.sciencedirect.com/science/article/pii/S0950584924001368
- Source type: peer-reviewed research paper
- Publication date: 2024
- Retrieval date: 2026-09-09
- Original takeaway: The paper treats the application-specific GUI test-oracle problem as a continuing barrier to automated defect detection. Keel therefore needs an `unclear` result that maps to `needs_human` instead of allowing a visual model or screenshot capture to imply correctness.
- Conditions and limitations: The evaluation concerns mobile GUI invariants and does not validate a particular VLM judge or Keel adapter.
- Program phase affected: Phase 8 UI verification and Phase 7 status semantics

### SRC-012: Static analysis warning actionability

- Title: Predicting Accurate and Actionable Static Analysis Warnings: An Experimental Approach
- URL / identifier: https://research.google/pubs/predicting-accurate-and-actionable-static-analysis-warnings-an-experimental-approach/
- Source type: peer-reviewed research paper (historical)
- Publication date: 2008
- Retrieval date: 2026-09-09
- Original takeaway: The study identifies both spurious warnings and legitimate warnings that developers do not act on, and evaluates warning-level signals for actionability. Keel should retain warning identity and lifecycle state rather than collapsing raw text into a successful process exit.
- Conditions and limitations: Historical evidence from the study's selected tools and projects. It does not justify assuming every diagnostic is a defect.
- Program phase affected: Phase 6 warning ledger and Phase 10 quality policy

### SRC-013: Static analyzer false positives and false negatives

- Title: An Empirical Study of False Negatives and Positives of Static Code Analyzers From the Perspective of Historical Issues
- URL / identifier: https://arxiv.org/abs/2408.13855
- Source type: research paper (preprint)
- Publication date: 2024-08-25
- Retrieval date: 2026-09-09
- Original takeaway: The study analyzes confirmed false-positive and false-negative issues in PMD, SpotBugs, and SonarQube and demonstrates multiple analyzer failure causes. Keel warning enforcement therefore needs baselines, deduplication, and review-visible waivers rather than an irreversible assumption that every warning is correct.
- Conditions and limitations: The analyzed Java-oriented tools do not represent Flutter, Rust, TypeScript, C/C++, .NET, Python, or package-manager diagnostics. Each parser still requires real fixtures.
- Program phase affected: Phase 6 warning lifecycle and Phase 10 false-positive policy

### SRC-014: cargo-mutants

- Title: cargo-mutants documentation
- URL / identifier: https://mutants.rs/
- Source type: official project documentation
- Publication date: continuously maintained; page does not state a fixed publication date
- Retrieval date: 2026-09-09
- Original takeaway: cargo-mutants injects candidate bugs and reports places where no test fails, providing a different quality signal from execution coverage or test counts. The project documentation also requires a reliable passing baseline before results are meaningful.
- Conditions and limitations: A missed mutant can be equivalent or unviable, and mutation results do not prove requirements, UI, security, or performance behavior.
- Program phase affected: Phase 10 mutation testing

### SRC-015: Claude Code MCP tool search

- Title: Connect Claude Code to tools via MCP
- URL / identifier: https://code.claude.com/docs/en/mcp
- Source type: official vendor documentation
- Publication date: continuously maintained; page does not state a fixed publication date
- Retrieval date: 2026-09-09
- Original takeaway: Current Claude Code documentation says MCP tool definitions are deferred and discovered on demand by default on supported deployments; `alwaysLoad` forces a server or tool into startup context and consumes context that would otherwise remain available. It also documents host and model fallbacks where tools load eagerly.
- Conditions and limitations: This is a current Claude Code contract only. Other Keel hosts may not support deferred loading, and vendor-reported savings must not be used as Keel measurements.
- Program phase affected: Phase 1 fixed-context ledger, Phase 11 instruction consolidation, and Phase 12 MCP profiles

### SRC-016: MCP tool-list contract

- Title: Model Context Protocol tools specification
- URL / identifier: https://modelcontextprotocol.io/specification/draft/server/tools
- Source type: official protocol specification (draft)
- Publication date: current draft; no fixed publication date
- Retrieval date: 2026-09-09
- Original takeaway: MCP servers expose a `tools/list` catalog with named schemas, and the current draft recommends deterministic order to support caching and prompt-cache stability. Keel's profile implementation must preserve protocol-valid discovery and deterministic catalogs.
- Conditions and limitations: Draft text can change and does not prescribe a particular host's eager/deferred model-context policy.
- Program phase affected: Phase 1 catalog measurement and Phase 12 profile behavior

### SRC-017: Current Keel impact-gate implementation

- Title: `impact_gate` implementation at merge commit `63281dd`
- URL / identifier: `rust/crates/keel/src/review/diff_gates.rs:561`
- Source type: local code
- Publication date: repository state retrieved 2026-09-09
- Retrieval date: 2026-09-09
- Original takeaway: The current owner returns `GateStatus::Pass` with `blocking: false` when graph creation returns `None`, while its detail says the impact check was skipped. The pasted PR #262 finding is confirmed on the current `main` path.
- Conditions and limitations: The current function defines impact as advisory and has no protected-surface policy. Phase 7 must trace the existing flow-check owner before selecting the blocking policy.
- Program phase affected: Phase 7 impact-gate correction

### SRC-018: Rust process-environment safety

- Title: Rust 2024 Edition Guide, Newly unsafe functions
- URL / identifier: https://doc.rust-lang.org/stable/edition-guide/rust-2024/newly-unsafe-functions.html
- Source type: official language documentation
- Publication date: continuously maintained; page does not state a fixed publication date
- Retrieval date: 2026-09-09
- Original takeaway: Rust documents that mutating process environment variables while other threads may run is unsafe on affected platforms and recommends avoiding that design. A parallel test that only needs an isolated Keel storage lane should pass an explicit unique home rather than depend on another test's temporary process-global override.
- Conditions and limitations: Rust documents `set_var` as sound on Windows, but that does not prevent application-level races in which parallel tests observe different logical home values. The observed Windows failure and the source trace establish that separate problem.
- Program phase affected: Phase 0 baseline stabilization

## Research boundary

The live pass covered every required topic. It does not establish Keel-specific quality, token, accuracy, or performance improvements. Those claims remain `unmeasured` until the program's deterministic fixtures and retained raw outputs produce repository-specific evidence.
