# CI/CD, Release Safety, and Secrets

## Use This Reference When

- Building or reviewing CI pipelines, deployment workflows, or GitOps promotion paths
- Adding image builds, package publishing, artifact provenance, or release gates
- Reworking secret handling, workload identity, or environment approvals
- Planning canary, blue-green, rolling, or manual rollback flows

## Delivery Rules of Thumb

- CI should prove code quality and artifact integrity before any deployment job can run.
- Promote immutable artifacts by digest or version; do not rebuild the same release separately per environment.
- Keep environment approvals and policy checks explicit so operators know exactly why a release can or cannot proceed.
- Treat database migrations, config changes, and background-worker rollouts as distinct release concerns.
- Prefer short-lived credentials and workload identity over long-lived repository secrets whenever the platform supports it.

## Secret and Supply-Chain Expectations

- Store secrets in managed secret systems or platform-native encrypted stores, not in repository files or broad CI variables.
- Scope secret access per job, environment, and role; remove shared administrator credentials from the pipeline.
- Scan dependencies, base images, and IaC definitions early enough that release decisions remain cheap.
- Preserve provenance, checksums, SBOMs, or equivalent audit signals where the platform supports them.

## Real-World Scenario: Rolling Out a Service With a Schema Change

- Build and test the artifact once, then promote the exact artifact through staging and production.
- Run forward-safe migrations before shifting traffic, and keep rollback semantics explicit if the schema change is irreversible.
- Canary the stateless service separately from stateful workers or migration jobs when their risk differs.
- Require health checks, error-rate thresholds, and an operator-owned abort path before full rollout.

## Validation Gates

- Required checks cover build, test, lint, security, packaging, and deployment policy as appropriate.
- Release artifacts are immutable and traceable to a specific commit, build, and approval path.
- Secret access is auditable, scoped, and revocable; removed secrets have a cleanup plan.
- Rollout and rollback steps are documented for web, worker, and migration components separately where needed.
- Deployment success criteria use runtime health signals, not just job completion status.

## Anti-Patterns

- Rebuilding artifacts in each environment and assuming they are equivalent
- Storing production credentials as long-lived repository secrets when federated identity is available
- Packing build, migration, deployment, and verification into one opaque job with no stop points
- Using mutable tags like latest for production promotion
- Skipping rollback planning because the team can just redeploy the last version
- Treating a passed deployment job as proof that background jobs, queues, and user traffic are healthy

## the harness Runtime Boundaries

- the harness can review workflow files, deployment scripts, promotion logic, and secret references in repository state.
- the harness cannot confirm OIDC trust, secret-store contents, artifact registry state, rollout-controller health, or external approval settings without runtime evidence.
- When CI or platform access is unavailable, require workflow runs, deployment event logs, artifact digests, and operator validation before claiming the release path is proven.

## Related References

- 00-devops-knowledge-map.md
- 10-iac-and-state-management.md
- 30-observability-incidents-and-sre.md
- 99-source-anchors.md
