//! Executable regression coverage for scoped memory and learning lifecycle.

use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture {
    base: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "keel-lifecycle-{label}-{}-{nonce}",
            std::process::id()
        ));
        let home = base.join("keel-home");
        fs::create_dir_all(&home).expect("create fixture home");
        Self { base, home }
    }

    fn home_arg(&self) -> &str {
        self.home.to_str().expect("UTF-8 fixture home")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_keel"))
        .args(args)
        .output()
        .expect("run keel")
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "expected JSON output: {error}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn assert_success(output: &Output, step: &str) {
    assert!(
        output.status.success(),
        "{step} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn research_record(fixture: &Fixture, scope: &str, answer: &str, extra: &[&str]) -> Value {
    let mut args = vec![
        "memory",
        "research-cache",
        "record",
        "--question",
        "current provider contract",
        "--answer",
        answer,
        "--source",
        "https://vendor.example/current",
        "--source-type",
        "official-doc",
        "--scope",
        scope,
        "--claude-home",
        fixture.home_arg(),
        "--json",
    ];
    let insert_at = args.len() - 3;
    args.splice(insert_at..insert_at, extra.iter().copied());
    let output = run(&args);
    assert_success(&output, "research-cache record");
    json(&output)
}

#[test]
fn scoped_replacement_and_expiry_define_current_truth() {
    let fixture = Fixture::new("scope");
    let first = research_record(&fixture, "workspace-a", "old answer", &[]);
    let old_id = first["record"]["id"].as_str().expect("old id").to_string();
    let other = research_record(&fixture, "workspace-b", "other scope answer", &[]);
    assert!(other["record"]["id"].is_string());
    let replacement = research_record(
        &fixture,
        "workspace-a",
        "current answer",
        &["--supersedes", &old_id],
    );
    let replacement_id = replacement["record"]["id"]
        .as_str()
        .expect("replacement id")
        .to_string();

    let old_path = fixture
        .home
        .join("memory/research-cache")
        .join(format!("{old_id}.json"));
    let old_record: Value = serde_json::from_str(
        &fs::read_to_string(&old_path).expect("superseded record remains durable"),
    )
    .expect("old record JSON");
    assert_eq!(old_record["state"], "superseded");
    assert_eq!(old_record["lifecycle"], "superseded");
    assert_eq!(old_record["supersededBy"], replacement_id);

    let scoped = run(&[
        "memory",
        "research-cache",
        "lookup",
        "--query",
        "current provider",
        "--scope",
        "workspace-a",
        "--claude-home",
        fixture.home_arg(),
        "--json",
    ]);
    assert_success(&scoped, "scoped lookup");
    let scoped_payload = json(&scoped);
    assert_eq!(scoped_payload["count"], 1);
    assert_eq!(scoped_payload["matches"][0]["answer"], "current answer");
    assert_eq!(
        scoped_payload["staleMatches"].as_array().map(Vec::len),
        Some(1)
    );

    let isolated = run(&[
        "memory",
        "research-cache",
        "lookup",
        "--query",
        "current provider",
        "--scope",
        "workspace-b",
        "--claude-home",
        fixture.home_arg(),
        "--json",
    ]);
    assert_success(&isolated, "other-scope lookup");
    let isolated_payload = json(&isolated);
    assert_eq!(isolated_payload["count"], 1);
    assert_eq!(
        isolated_payload["matches"][0]["answer"],
        "other scope answer"
    );

    let expired = run(&[
        "memory",
        "research-cache",
        "expire",
        "--id",
        &replacement_id,
        "--claude-home",
        fixture.home_arg(),
        "--json",
    ]);
    assert_success(&expired, "expire replacement");
    assert_eq!(json(&expired)["entry"]["lifecycle"], "expired");

    let after_expiry = run(&[
        "memory",
        "research-cache",
        "lookup",
        "--query",
        "current provider",
        "--scope",
        "workspace-a",
        "--claude-home",
        fixture.home_arg(),
        "--json",
    ]);
    assert_success(&after_expiry, "lookup after expiry");
    let after_payload = json(&after_expiry);
    assert_eq!(after_payload["count"], 0);
    assert_eq!(
        after_payload["staleMatches"].as_array().map(Vec::len),
        Some(2)
    );
}

#[test]
fn evidence_is_required_for_promotion_and_penalties_quarantine() {
    let fixture = Fixture::new("learning");
    let home = fixture.home_arg();
    let hunch = run(&[
        "memory",
        "instincts",
        "record",
        "--trigger",
        "unverified hunch",
        "--guidance",
        "do not promote this",
        "--claude-home",
        home,
    ]);
    assert_success(&hunch, "record unverified hunch");
    for _ in 0..2 {
        let reinforced = run(&[
            "memory",
            "instincts",
            "reinforce",
            "--trigger",
            "unverified hunch",
            "--claude-home",
            home,
        ]);
        assert_success(&reinforced, "reinforce hunch");
    }
    let promoted_hunch = run(&[
        "memory",
        "instincts",
        "promote",
        "--json",
        "--claude-home",
        home,
    ]);
    assert_success(&promoted_hunch, "promote hunch");
    let hunch_payload = json(&promoted_hunch);
    assert_eq!(hunch_payload["count"], 0);
    assert_eq!(hunch_payload["excludedWithoutEvidence"], 1);

    let verified = run(&[
        "memory",
        "instincts",
        "record",
        "--trigger",
        "verified procedure",
        "--guidance",
        "run the checked procedure",
        "--evidence",
        "three independent successful runs",
        "--scope",
        "workspace-a",
        "--claude-home",
        home,
    ]);
    assert_success(&verified, "record evidence-backed lesson");
    for _ in 0..2 {
        let reinforced = run(&[
            "memory",
            "instincts",
            "reinforce",
            "--trigger",
            "verified procedure",
            "--claude-home",
            home,
        ]);
        assert_success(&reinforced, "reinforce evidence-backed lesson");
    }
    let promoted = run(&[
        "memory",
        "instincts",
        "promote",
        "--scope",
        "workspace-a",
        "--json",
        "--claude-home",
        home,
    ]);
    assert_success(&promoted, "promote evidence-backed lesson");
    let promoted_payload = json(&promoted);
    assert_eq!(promoted_payload["count"], 1);
    assert_eq!(promoted_payload["promoted"][0]["evidenceCount"], "1");
    assert_eq!(promoted_payload["promoted"][0]["scope"], "workspace-a");

    for _ in 0..2 {
        let penalized = run(&[
            "memory",
            "instincts",
            "penalize",
            "--trigger",
            "verified procedure",
            "--claude-home",
            home,
        ]);
        assert_success(&penalized, "penalize lesson");
    }
    let listed = run(&[
        "memory",
        "instincts",
        "list",
        "--json",
        "--claude-home",
        home,
    ]);
    assert_success(&listed, "list lifecycle records");
    let listed_payload = json(&listed);
    let records = listed_payload["records"].as_array().expect("records array");
    let verified_record = records
        .iter()
        .find(|record| record["trigger"] == "verified procedure")
        .expect("verified record");
    assert_eq!(verified_record["lifecycle"], "quarantined");
    assert_eq!(verified_record["evaluationStatus"], "regressed");
}

#[test]
fn instinct_admission_and_list_projection_are_bounded_without_eviction() {
    let fixture = Fixture::new("bounds");
    let store = fixture.home.join("memory/instincts");
    fs::create_dir_all(&store).expect("create instinct store");
    for index in 0..200 {
        let id = format!("seed-{index}");
        fs::write(
            store.join(format!("{id}.json")),
            format!(
                "{{\"id\":\"{id}\",\"trigger\":\"seed {index}\",\"guidance\":\"preserve\",\"confidence\":\"1\"}}"
            ),
        )
        .expect("seed bounded records");
    }
    let rejected = run(&[
        "memory",
        "instincts",
        "record",
        "--trigger",
        "one-too-many",
        "--guidance",
        "must remain rejected",
        "--claude-home",
        fixture.home_arg(),
    ]);
    assert_eq!(rejected.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("record limit"));
    assert!(store.join("seed-0.json").exists());
    assert!(!store.join("one-too-many.json").exists());

    let listed = run(&[
        "memory",
        "instincts",
        "list",
        "--json",
        "--claude-home",
        fixture.home_arg(),
    ]);
    assert_success(&listed, "bounded instinct list");
    let payload = json(&listed);
    assert_eq!(payload["count"], 200);
    assert_eq!(payload["totalCount"], 200);
    assert_eq!(payload["truncated"], false);
    assert_eq!(payload["records"].as_array().map(Vec::len), Some(200));
}
