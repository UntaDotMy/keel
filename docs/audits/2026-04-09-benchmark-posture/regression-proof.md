# Regression Proof

Rerun these commands from the repository root to verify the closed findings remain closed:

```bash
cargo test --workspace
cargo build --release --bin keel
./target/release/keel validate --repo-root . --profile smoke
```

Expected result:

- the benchmark suite keeps linking to the published benchmark posture audit bundle
- the shared harness doc keeps the benchmark-posture claim bundled and honest about peer-repo evidence
- the repo-wide Rust proof stays green while the benchmark trust-artifact surfaces remain aligned
