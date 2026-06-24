//! Purpose: Workflow routing rules table and keyword matching
//! Caller: workflow.rs (run_workflow_route)
//! Dependencies: None
//! Main Functions: match_routing_rule, first_matching_keyword
//! Side Effects: None, pure data + lookup

pub(super) struct RoutingRule {
    pub(super) keywords: &'static [&'static str],
    pub(super) specialist: &'static str,
    pub(super) reason: &'static str,
}

pub(super) const DEFAULT_ROUTE: RoutingRule = RoutingRule {
    keywords: &[],
    specialist: "software-development-life-cycle",
    reason: "default lane for cross-domain coordination and sequencing",
};

pub(super) const ROUTING_RULES: &[RoutingRule] = &[
    RoutingRule {
        keywords: &[
            "audit",
            "review",
            "reviewer",
            "production-ready",
            "production ready",
            "quality gate",
            "release risk",
            "gap analysis",
            "release readiness",
        ],
        specialist: "reviewer",
        reason: "production readiness and final quality gate",
    },
    RoutingRule {
        keywords: &[
            "preserve existing",
            "preserve-existing-flow",
            "brownfield",
            "existing flow",
            "owner trace",
            "source of truth",
        ],
        specialist: "preserve-existing-flow",
        reason: "brownfield ownership tracing before behavior change",
    },
    RoutingRule {
        keywords: &[
            "git",
            "branch",
            "rebase",
            "merge conflict",
            "force push",
            "worktree",
            "pull request",
            "gh pr",
            "github pr",
            "pr body",
            "commit message",
        ],
        specialist: "git-expert",
        reason: "git workflow, PR, or branching operations",
    },
    RoutingRule {
        keywords: &[
            "security",
            "vulnerability",
            "threat model",
            "threat",
            "compliance",
            "soc2",
            "gdpr",
            "owasp",
            "secret",
            "authorization",
            "rbac",
        ],
        specialist: "security-and-compliance-auditor",
        reason: "security, threat modeling, or compliance review",
    },
    // Build-side identity work. Placed AFTER the security rule (which reviews
    // auth) so a distinctive security verb like "threat model" still wins, while
    // a build ask with no security verb ("implement oauth2/oidc/login") routes
    // here instead of falling to the default lane. The auditor finds; this builds.
    RoutingRule {
        keywords: &[
            "oauth2",
            "oauth",
            "oidc",
            "openid",
            "openid connect",
            "login flow",
            "login",
            "sso",
            "single sign-on",
            "saml",
            "passkey",
            "webauthn",
            "mfa",
            "jwt",
            "refresh token",
            "session token",
            "identity provider",
            "auth",
            "authentication",
        ],
        specialist: "authentication-and-identity",
        reason: "login, session, token, or SSO/identity build",
    },
    RoutingRule {
        keywords: &[
            "test",
            "tests",
            "tdd",
            "playwright",
            "cypress",
            "e2e",
            "regression",
            "coverage",
            "fixture",
            "qa",
        ],
        specialist: "qa-and-automation-engineer",
        reason: "test strategy, automation, or release ladder validation",
    },
    // Narrow infra-adjacent specialists, placed before cloud-and-devops-expert
    // so their distinctive tokens win over its broad "cloud"/"pipeline" tokens
    // (first-match-wins). cloud-cost-and-finops must beat "cloud"; the data/ML
    // ETL tokens must beat "pipeline".
    RoutingRule {
        keywords: &[
            "finops",
            "cloud cost",
            "cost optimization",
            "rightsizing",
            "reserved instance",
            "savings plan",
            "committed use",
            "spot instance",
            "cost allocation",
            "budget guardrail",
            "unit economics",
            "infracost",
        ],
        specialist: "cloud-cost-and-finops",
        reason: "cloud cost estimation, rightsizing, or commitment planning",
    },
    RoutingRule {
        keywords: &[
            "observability",
            "slo",
            "sli",
            "error budget",
            "incident",
            "incident response",
            "postmortem",
            "post-mortem",
            "on-call",
            "oncall",
            "paging",
            "alerting",
            "runbook",
            "opentelemetry",
            "otel",
            "telemetry",
            "burn rate",
        ],
        specialist: "observability-and-incident-response",
        reason: "telemetry, SLO/SLI, alerting, or incident response",
    },
    RoutingRule {
        keywords: &[
            "etl",
            "elt",
            "data pipeline",
            "etl pipeline",
            "ml pipeline",
            "data warehouse",
            "lakehouse",
            "dbt",
            "airflow",
            "dagster",
            "prefect",
            "feature engineering",
            "feature store",
            "model training",
            "model serving",
            "drift monitoring",
            "train/serve",
            "mlops",
            "machine learning",
        ],
        specialist: "data-and-ml-engineering",
        reason: "data engineering pipelines or the ML/MLOps lifecycle",
    },
    RoutingRule {
        keywords: &[
            "i18n",
            "l10n",
            "internationalization",
            "localization",
            "localisation",
            "translation",
            "message catalog",
            "icu messageformat",
            "pluralization",
            "rtl",
            "bidi",
            "locale",
            "pseudo-localization",
        ],
        specialist: "internationalization-and-localization",
        reason: "message catalogs, locale formatting, or RTL/bidi correctness",
    },
    RoutingRule {
        keywords: &[
            "dependency upgrade",
            "dependency update",
            "dependencies",
            "sbom",
            "supply chain",
            "supply-chain",
            "lockfile",
            "dependabot",
            "renovate",
            "transitive dependency",
            "typosquat",
            "provenance",
            "pinning strategy",
        ],
        specialist: "dependency-and-supply-chain",
        reason: "dependency upgrades, lockfile hygiene, or supply-chain provenance",
    },
    RoutingRule {
        keywords: &[
            "deploy",
            "deployment",
            "ci/cd",
            "pipeline",
            "kubernetes",
            "k8s",
            "terraform",
            "pulumi",
            "infrastructure",
            "cloud",
            "aws",
            "gcp",
            "azure",
            "docker",
            "helm",
            "rollout",
            "rollback",
        ],
        specialist: "cloud-and-devops-expert",
        reason: "infrastructure, CI/CD, or deployment ownership",
    },
    RoutingRule {
        keywords: &[
            "stripe",
            "payment intent",
            "payment intents",
            "checkout session",
            "subscription billing",
            "webhook signature",
            "stripe webhook",
            "stripe connect",
            "chargeback",
            "dispute",
            "refund",
            "sca",
            "3ds",
            "pci",
        ],
        specialist: "stripe-integration",
        reason: "Stripe payments, subscriptions, webhooks, or PCI scope",
    },
    RoutingRule {
        keywords: &[
            "websocket",
            "web socket",
            "socket.io",
            "socketio",
            "server-sent events",
            "sse",
            "realtime",
            "real-time",
            "presence",
            "fan-out",
            "fanout",
            "long-polling",
            "webrtc data channel",
        ],
        specialist: "websocket-realtime-design",
        reason: "WebSocket, SSE, or realtime fan-out architecture",
    },
    RoutingRule {
        keywords: &[
            "postgres migration",
            "postgresql migration",
            "schema migration",
            "alter table",
            "expand and contract",
            "expand-and-contract",
            "backfill",
            "create index concurrently",
            "lock_timeout",
            "statement_timeout",
            "not valid constraint",
        ],
        specialist: "postgres-migration-safety",
        reason: "PostgreSQL migration, lock analysis, or backfill strategy",
    },
    RoutingRule {
        keywords: &[
            "react performance",
            "react perf",
            "react re-render",
            "react rerender",
            "memoization",
            "usememo",
            "usecallback",
            "react.memo",
            "hydration mismatch",
            "suspense waterfall",
            "bundle size",
            "code splitting",
            "core web vitals",
            "lcp",
            "inp",
            "react profiler",
        ],
        specialist: "react-performance-audit",
        reason: "React render audit, bundle triage, or Core Web Vitals",
    },
    RoutingRule {
        keywords: &[
            "api contract",
            "openapi",
            "swagger",
            "api version",
            "breaking change",
            "schema evolution",
            "asyncapi",
            "json schema",
            "grpc proto",
            "proto file",
            "graphql schema",
            "idempotency key",
            "error taxonomy",
            "cursor pagination",
        ],
        specialist: "api-contract-design",
        reason: "API contract design, schema evolution, or breaking-change review",
    },
    RoutingRule {
        keywords: &[
            "api",
            "microservice",
            "microservices",
            "database",
            "schema",
            "queue",
            "kafka",
            "postgres",
            "postgresql",
            "mysql",
            "mongodb",
            "redis",
            "graphql",
            "rest endpoint",
        ],
        specialist: "backend-and-data-architecture",
        reason: "backend service, API, or data architecture",
    },
    RoutingRule {
        keywords: &[
            "mobile",
            "ios",
            "android",
            "swift",
            "kotlin",
            "react native",
            "flutter",
            "app store",
        ],
        specialist: "mobile-development-life-cycle",
        reason: "mobile platform development",
    },
    RoutingRule {
        keywords: &[
            "frontend", "browser", "react", "vue", "svelte", "next.js", "nextjs", "html", "css",
            "spa", "webpage", "website", "web app",
        ],
        specialist: "web-development-life-cycle",
        reason: "web application development",
    },
    RoutingRule {
        keywords: &[
            "ux",
            "user research",
            "journey",
            "funnel",
            "usability",
            "user experience",
            "user testing",
        ],
        specialist: "ux-research-and-experience-strategy",
        reason: "user experience strategy and research",
    },
    RoutingRule {
        keywords: &[
            "ui",
            "design system",
            "design tokens",
            "responsive",
            "accessibility",
            "wcag",
            "layout",
            "component library",
        ],
        specialist: "ui-design-systems-and-responsive-interfaces",
        reason: "UI design system or responsive interface",
    },
    RoutingRule {
        keywords: &[
            "memory health",
            "memory status",
            "learning recap",
            "what did i learn",
            "what did you learn",
            "memory growth",
        ],
        specialist: "memory-status-reporter",
        reason: "memory health, learning, and mistake reporting",
    },
];

pub(super) fn match_routing_rule(request: &str) -> &'static RoutingRule {
    let lowercased = request.to_lowercase();
    for rule in ROUTING_RULES {
        for keyword in rule.keywords {
            if request_contains_keyword(&lowercased, keyword) {
                return rule;
            }
        }
    }
    &DEFAULT_ROUTE
}

pub(super) fn first_matching_keyword(request: &str, rule: &RoutingRule) -> &'static str {
    let lowercased = request.to_lowercase();
    for keyword in rule.keywords {
        if request_contains_keyword(&lowercased, keyword) {
            return keyword;
        }
    }
    ""
}

fn request_contains_keyword(request_lowercased: &str, keyword: &str) -> bool {
    if keyword.contains(' ') {
        return request_lowercased.contains(keyword);
    }
    request_lowercased
        .split(|character: char| {
            !character.is_alphanumeric() && character != '-' && character != '_'
        })
        .any(|token| token == keyword)
}
