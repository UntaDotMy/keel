use keel::proxy::context::{
    project_scoped, CacheClass, ContextFirewall, ContextPolicy, ContextSource, ProjectionInput,
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
