# Regression Proof

Rerun these commands from the repository root to verify the competitive audit bundle and roadmap stay aligned:

```bash
cargo test --workspace
cargo build --release --bin keel
./target/release/keel validate --repo-root . --profile smoke
```

Expected result:

- README keeps linking to the published competitive audit bundle.
- The benchmark comparison page keeps the per-surface scores explicit and avoids a blended overall leaderboard claim.
- The roadmap keeps the new competitive gaps visible while preserving the completed roadmap archive.
- Repo-wide Rust proof stays green while the new audit-bundle surfaces remain aligned.
