use keel::proxy::context::{
    project_scoped, schedule_turn_scoped, CacheClass, ContextFirewall, ContextPolicy,
    ContextSource, ProjectionInput,
};

#[test]
fn projection_is_bounded_and_keeps_a_recovery_pointer() {
    let mut firewall = ContextFirewall::new(ContextPolicy::with_max_tokens(24));
    let input = ProjectionInput::new(
        ContextSource::CommandOutput,
        (1..=80)
            .map(|index| format!("diagnostic line {index}"))
            .collect::<Vec<_>>()
            .join("\n"),
        Some("20260909-raw"),
        "workspace-a",
        "session-a",
    )
    .with_request_id("request-a")
    .with_cache_class(CacheClass::Dynamic);

    let projection = firewall
        .project(input)
        .expect("large output should be reduced, not rejected");

    assert!(projection.truncated);
    assert!(projection.token_count <= 24);
    assert_eq!(projection.raw_artifact_id.as_deref(), Some("20260909-raw"));
    assert!(projection.summary.contains("20260909-raw"));
    let provenance_digest = projection
        .provenance_id
        .strip_prefix("prov-fnv1a:")
        .expect("stable provenance prefix");
    assert_eq!(provenance_digest.len(), 64);
    assert!(provenance_digest
        .chars()
        .all(|character| character.is_ascii_hexdigit()));
}

#[test]
fn duplicate_projection_is_suppressed_within_a_session_namespace() {
    let mut firewall = ContextFirewall::default();
    let input = ProjectionInput::new(
        ContextSource::Memory,
        "same durable evidence",
        Some("raw-memory"),
        "workspace-a",
        "session-a",
    );

    assert!(firewall.project(input.clone()).is_ok());
    assert!(firewall
        .project_optional(input)
        .expect("duplicate detection should be explicit")
        .is_none());
}

#[test]
fn duplicate_content_is_suppressed_even_when_sources_differ() {
    let mut firewall = ContextFirewall::default();
    firewall
        .project(ProjectionInput::new(
            ContextSource::Memory,
            "shared evidence",
            None::<String>,
            "workspace-a",
            "session-a",
        ))
        .expect("first source should be accepted");

    let duplicate = firewall.project_optional(ProjectionInput::new(
        ContextSource::Task,
        "shared evidence",
        None::<String>,
        "workspace-a",
        "session-a",
    ));
    assert!(duplicate
        .expect("cross-source duplicate detection should be explicit")
        .is_none());
}

#[test]
fn empty_projection_does_not_poison_session_dedupe() {
    let mut firewall = ContextFirewall::default();
    let empty = ProjectionInput::new(
        ContextSource::CommandOutput,
        "\n  \n",
        None::<String>,
        "workspace-empty",
        "session-empty",
    );
    let first = firewall
        .project(empty.clone())
        .expect("an empty result is a valid bounded projection");
    assert_eq!(first.summary, "");

    let second = firewall
        .project(empty)
        .expect("repeated empty results must not be treated as duplicate evidence");
    assert_eq!(second.summary, "");
}

#[test]
fn duplicate_content_is_suppressed_even_when_request_ids_differ() {
    let mut firewall = ContextFirewall::default();
    firewall
        .project(
            ProjectionInput::new(
                ContextSource::CommandOutput,
                "same command result",
                None::<String>,
                "workspace-a",
                "session-a",
            )
            .with_request_id("request-1"),
        )
        .expect("first request should be accepted");
    let duplicate = firewall.project_optional(
        ProjectionInput::new(
            ContextSource::CommandOutput,
            "same command result",
            None::<String>,
            "workspace-a",
            "session-a",
        )
        .with_request_id("request-2"),
    );
    assert!(duplicate
        .expect("duplicate detection should be explicit")
        .is_none());
}

#[test]
fn budget_failure_never_returns_an_unbounded_raw_fallback() {
    let mut firewall = ContextFirewall::new(ContextPolicy::with_max_tokens(0));
    let result = firewall.project(ProjectionInput::new(
        ContextSource::McpTool,
        "raw tool response that must not leak",
        Some("raw-tool"),
        "workspace-a",
        "session-a",
    ));

    let error = result.expect_err("zero budget must fail closed");
    let rendered = error.to_string();
    assert!(rendered.contains("budget"));
    assert!(!rendered.contains("raw tool response"));
}

#[test]
fn low_budget_projection_keeps_critical_failure_identity() {
    let mut firewall = ContextFirewall::new(ContextPolicy::with_max_tokens(32));
    let content = [
        "ordinary setup output that should not win priority",
        "another ordinary line before the failure",
        "error: critical-test-42 failed at src/lib.rs:40",
        "exit status: 1",
    ]
    .join("\n");
    let projection = firewall
        .project(ProjectionInput::new(
            ContextSource::Error,
            content,
            Some("raw-critical-error"),
            "workspace-critical",
            "session-critical",
        ))
        .expect("critical failure projection should remain bounded");

    assert!(projection.truncated);
    assert!(projection.summary.contains("critical-test-42"));
    assert!(projection.summary.contains("failed"));
    assert!(projection.token_count <= 32);
}

#[test]
fn projection_rejects_cross_namespace_artifact_ids() {
    let mut firewall = ContextFirewall::default();
    let result = firewall.project(ProjectionInput::new(
        ContextSource::CommandOutput,
        "safe result",
        Some("../other-session/raw"),
        "workspace-a",
        "session-a",
    ));

    assert!(result
        .expect_err("artifact ids must not escape their namespace")
        .to_string()
        .contains("artifact"));
}

#[test]
fn shared_gateway_suppresses_duplicate_payloads_per_session_workspace() {
    let workspace = format!("gateway-test-workspace-{}", std::process::id());
    let session = format!("gateway-test-session-{}", std::process::id());
    let input = ProjectionInput::new(
        ContextSource::McpTool,
        "shared gateway evidence",
        None::<String>,
        &workspace,
        &session,
    );
    assert!(project_scoped(ContextPolicy::default(), input.clone()).is_ok());
    assert!(project_scoped(ContextPolicy::default(), input)
        .expect_err("same session/workspace must dedupe")
        .to_string()
        .contains("duplicate"));
    assert!(project_scoped(
        ContextPolicy::default(),
        ProjectionInput::new(
            ContextSource::McpTool,
            "shared gateway evidence",
            None::<String>,
            &workspace,
            format!("{session}-other"),
        )
    )
    .is_ok());
}

#[test]
fn shared_gateway_applies_the_current_surface_budget() {
    let workspace = format!("gateway-policy-workspace-{}", std::process::id());
    let session = format!("gateway-policy-session-{}", std::process::id());
    let narrow = ProjectionInput::new(
        ContextSource::Memory,
        "memory pointer",
        None::<String>,
        &workspace,
        &session,
    );
    project_scoped(ContextPolicy::with_max_tokens(2), narrow)
        .expect("short memory pointer should fit the narrow budget");

    let command = ProjectionInput::new(
        ContextSource::CommandOutput,
        "command output with enough context for the wider surface budget",
        None::<String>,
        &workspace,
        &session,
    );
    let projection = project_scoped(ContextPolicy::with_max_tokens(24), command)
        .expect("the later command surface should use its own budget");
    assert!(projection.token_count <= 24);
}

#[test]
fn research_and_task_projections_respect_surface_budgets() {
    let mut firewall = ContextFirewall::default();
    let research = ProjectionInput::new(
        ContextSource::Research,
        "verified claim: API version 2026-07-28 is active and supported",
        None::<String>,
        "workspace-research",
        "session-research",
    );
    let proj = firewall
        .project(research)
        .expect("research projection must succeed");
    assert_eq!(proj.source, ContextSource::Research);
    assert_eq!(proj.source.priority_tier(), 4);

    let task = ProjectionInput::new(
        ContextSource::Task,
        "task PLAN-123 TODO-1: implement modern MCP request header routing",
        None::<String>,
        "workspace-task",
        "session-task",
    );
    let proj_task = firewall
        .project(task)
        .expect("task projection must succeed");
    assert_eq!(proj_task.source, ContextSource::Task);
    assert_eq!(proj_task.source.priority_tier(), 6);
}

#[test]
fn nine_tier_priority_scheduler_prioritizes_higher_tier_and_omits_lower() {
    let mut firewall = ContextFirewall::new(ContextPolicy::with_max_tokens(100));

    let tier1_warning = ProjectionInput::new(
        ContextSource::Warning,
        "SECURITY: sandbox access denied for path /etc/passwd",
        None::<String>,
        "workspace-sched",
        "session-sched",
    );
    let tier2_error = ProjectionInput::new(
        ContextSource::Error,
        "FATAL: build failed with 2 compilation errors in src/main.rs",
        None::<String>,
        "workspace-sched",
        "session-sched",
    );
    let tier4_research = ProjectionInput::new(
        ContextSource::Research,
        "Research finding: official documentation states header Mcp-Method is mandatory",
        None::<String>,
        "workspace-sched",
        "session-sched",
    );
    let tier9_instruction = ProjectionInput::new(
        ContextSource::Instruction,
        "Optional repetitive advice: remember to write tests and keep diffs small and clean",
        None::<String>,
        "workspace-sched",
        "session-sched",
    );

    // Candidates presented out of priority order
    let candidates = vec![
        tier9_instruction,
        tier4_research,
        tier1_warning,
        tier2_error,
    ];

    // Schedule with generous budget: all 4 scheduled in priority order (1, 2, 4, 9)
    let res = firewall
        .schedule_turn(candidates.clone(), 500)
        .expect("schedule turn must succeed");
    assert_eq!(res.projections.len(), 4);
    assert_eq!(res.projections[0].source, ContextSource::Warning);
    assert_eq!(res.projections[1].source, ContextSource::Error);
    assert_eq!(res.projections[2].source, ContextSource::Research);
    assert_eq!(res.projections[3].source, ContextSource::Instruction);
    assert_eq!(res.omitted_count, 0);

    // Schedule with tight budget: higher tiers included, lower omitted
    let mut tight_firewall = ContextFirewall::new(ContextPolicy::with_max_tokens(30));
    let tight_res = tight_firewall
        .schedule_turn(candidates, 30)
        .expect("tight schedule must succeed");
    assert!(tight_res.omitted_count > 0);
    assert!(tight_res.total_visible_tokens <= 30);
    // Highest priority tier (Warning, tier 1) must be in the projections
    assert_eq!(tight_res.projections[0].source, ContextSource::Warning);
}

#[test]
fn schedule_turn_scoped_preserves_session_identity_and_priority_order() {
    let tier1_warning = ProjectionInput::new(
        ContextSource::Warning,
        "compiler warning in main.rs",
        None::<String>,
        "ws-test",
        "session-turn-sched",
    );
    let tier9_instruction = ProjectionInput::new(
        ContextSource::Instruction,
        "remember to run reviewer",
        None::<String>,
        "ws-test",
        "session-turn-sched",
    );
    let candidates = vec![tier9_instruction, tier1_warning];
    let res = schedule_turn_scoped(500, "ws-test", "session-turn-sched", candidates)
        .expect("schedule turn scoped must succeed");
    assert_eq!(res.projections.len(), 2);
    assert_eq!(res.projections[0].source, ContextSource::Warning);
    assert_eq!(res.projections[1].source, ContextSource::Instruction);
}
