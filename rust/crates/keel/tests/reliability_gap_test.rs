//! Purpose: Exercise reliability-plan gaps at the raw persistence and package lifecycle boundaries.
//! Caller: Cargo integration-test harness for the keel crate.
//! Dependencies: Public RawStore APIs, the built keel binary, and native manager commands.
//! Main Functions: Concurrent persistence/read verification and packaged install smoke coverage.
//! Side Effects: Creates and removes unique temporary trees and launches isolated keel processes.

use keel::proxy::raw_store::{RawRun, RawStore, RunMeta};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

struct TemporaryTree(PathBuf);

impl Drop for TemporaryTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("workspace repository root")
        .to_path_buf()
}

fn keel_binary() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_keel") {
        return PathBuf::from(path);
    }

    let mut path = env::current_exe().expect("resolve integration test executable");
    path.pop();
    path.pop();
    path.push(if cfg!(windows) { "keel.exe" } else { "keel" });
    path
}

fn temporary_tree(prefix: &str) -> TemporaryTree {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let root = env::temp_dir().join(format!("{prefix}-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&root).expect("create isolated temporary tree");
    TemporaryTree(root)
}

fn raw_meta(raw_id: &str) -> RunMeta {
    RunMeta {
        raw_id: raw_id.to_string(),
        command: "reliability-test".to_string(),
        program: "reliability-test".to_string(),
        args: Vec::new(),
        cwd: PathBuf::from("."),
        started_at: 1,
        duration_ms: 0,
        exit_code: 0,
        adapter_name: "reliability-test".to_string(),
        raw_path: PathBuf::new(),
        compact_path: PathBuf::new(),
        agent: "reliability-test".to_string(),
        workspace: PathBuf::from("reliability-test"),
        stdout_bytes: 0,
        stderr_bytes: 0,
        compact_stdout_bytes: 0,
        compact_stderr_bytes: 0,
        estimated_tokens_before: 0,
        estimated_tokens_after: 0,
        estimated_tokens_saved: 0,
        savings_pct: 0.0,
        compacted: false,
    }
}

fn expected_stdout(raw_id: &str) -> Vec<u8> {
    format!("stdout:{raw_id}").into_bytes()
}

fn assert_success(output: &Output, operation: &str) {
    assert!(
        output.status.success(),
        "{operation} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_cli(binary: &Path, arguments: &[&OsStr], current_directory: Option<&Path>) -> Output {
    let mut command = Command::new(binary);
    command.args(arguments);
    if let Some(directory) = current_directory {
        command.current_dir(directory);
    }
    command.output().expect("launch keel process")
}

fn stage_release_bundle(bundle_root: &Path, binary: &Path) {
    for relative_path in [
        "AGENTS.md",
        "README.md",
        "00-skill-routing-and-escalation.md",
        "reviewer/SKILL.md",
    ] {
        let source = repository_root().join(relative_path);
        let target = bundle_root.join(relative_path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("create release bundle directory");
        }
        fs::copy(source, target).expect("stage release bundle file");
    }

    let executable_name = if cfg!(windows) { "keel.exe" } else { "keel" };
    fs::copy(binary, bundle_root.join(executable_name)).expect("stage release executable");
    fs::write(
        bundle_root.join("keel-release-manifest.json"),
        br#"{
  "package_kind": "release",
  "repository_slug": "UntaDotMy/keel",
  "release_tag": "v-reliability-test",
  "build_version": "0.1.0-reliability-test"
}
"#,
    )
    .expect("write release manifest");
}

fn metadata_value<'a>(metadata: &'a str, key: &str) -> Option<&'a str> {
    metadata.lines().find_map(|line| {
        let (line_key, line_value) = line.split_once('=')?;
        (line_key == key).then_some(line_value)
    })
}

#[test]
fn raw_store_survives_concurrent_writers_and_readers() {
    const WRITER_COUNT: usize = 12;
    const RUNS_PER_WRITER: usize = 8;
    const READER_COUNT: usize = 4;
    const READER_PASSES: usize = 8;

    let tree = temporary_tree("keel-raw-contention");
    let store = Arc::new(RawStore::with_root(tree.0.join("raw-output")));
    let start_barrier = Arc::new(Barrier::new(WRITER_COUNT + READER_COUNT));
    let mut workers = Vec::new();

    for worker_index in 0..WRITER_COUNT {
        let store = Arc::clone(&store);
        let start_barrier = Arc::clone(&start_barrier);
        workers.push(thread::spawn(move || -> io::Result<()> {
            start_barrier.wait();
            for run_index in 0..RUNS_PER_WRITER {
                let raw_id = format!("contention-{worker_index:02}-{run_index:02}");
                let mut metadata = raw_meta(&raw_id);
                let stdout = expected_stdout(&raw_id);
                store
                    .save(
                        &mut metadata,
                        &RawRun {
                            stdout,
                            stderr: b"stderr".to_vec(),
                            exit_code: 0,
                        },
                    )
                    .map_err(|error| {
                        io::Error::new(error.kind(), format!("save {raw_id}: {error}"))
                    })?;
            }
            Ok(())
        }));
    }

    for _ in 0..READER_COUNT {
        let store = Arc::clone(&store);
        let start_barrier = Arc::clone(&start_barrier);
        workers.push(thread::spawn(move || -> io::Result<()> {
            start_barrier.wait();
            for _ in 0..READER_PASSES {
                for entry in store.list().map_err(|error| {
                    io::Error::new(error.kind(), format!("list during contention: {error}"))
                })? {
                    let stdout = store
                        .read_file(&entry.raw_id, "stdout.log")
                        .map_err(|error| {
                            io::Error::new(
                                error.kind(),
                                format!("read {} during contention: {error}", entry.raw_id),
                            )
                        })?;
                    if stdout != expected_stdout(&entry.raw_id) {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("stdout mismatch for {}", entry.raw_id),
                        ));
                    }
                }
                thread::yield_now();
            }
            Ok(())
        }));
    }

    for worker in workers {
        worker
            .join()
            .expect("raw-store contention worker must not panic")
            .expect("raw-store contention operation must succeed");
    }

    let entries = store.list().expect("list committed raw entries");
    assert_eq!(
        entries.len(),
        WRITER_COUNT * RUNS_PER_WRITER,
        "every committed writer entry must be visible exactly once"
    );
    for entry in entries {
        assert_eq!(
            store
                .load_meta(&entry.raw_id)
                .expect("load committed metadata")
                .raw_id,
            entry.raw_id
        );
        assert_eq!(
            store
                .read_file(&entry.raw_id, "stderr.log")
                .expect("read committed stderr"),
            b"stderr"
        );
    }
}

#[test]
fn raw_store_prune_removes_stale_entries_during_a_writer_race() {
    let tree = temporary_tree("keel-raw-retention");
    let raw_root = tree.0.join("raw-output");
    let stale = raw_root.join("2001-02-03").join("stale-entry");
    let fresh = raw_root.join("2099-01-01").join("fresh-entry");
    fs::create_dir_all(&stale).expect("create stale fixture");
    fs::create_dir_all(&fresh).expect("create fresh fixture");
    fs::write(stale.join("stdout.log"), b"old").expect("write stale fixture");
    fs::write(fresh.join("stdout.log"), b"new").expect("write fresh fixture");

    let store = Arc::new(RawStore::with_root(raw_root));
    let start_barrier = Arc::new(Barrier::new(2));
    let prune_store = Arc::clone(&store);
    let prune_barrier = Arc::clone(&start_barrier);
    let pruner = thread::spawn(move || -> io::Result<usize> {
        prune_barrier.wait();
        prune_store.prune_older_than(30)
    });

    let writer_store = Arc::clone(&store);
    let writer_barrier = Arc::clone(&start_barrier);
    let writer = thread::spawn(move || -> io::Result<()> {
        writer_barrier.wait();
        let raw_id = "retention-writer";
        let mut metadata = raw_meta(raw_id);
        writer_store
            .save(
                &mut metadata,
                &RawRun {
                    stdout: expected_stdout(raw_id),
                    stderr: Vec::new(),
                    exit_code: 0,
                },
            )
            .map_err(|error| io::Error::new(error.kind(), format!("save {raw_id}: {error}")))
    });

    let removed = pruner
        .join()
        .expect("retention pruner must not panic")
        .expect("retention prune must succeed");
    writer
        .join()
        .expect("retention writer must not panic")
        .expect("retention writer must succeed");

    assert_eq!(removed, 1, "only the stale logical day should be pruned");
    assert!(!stale.exists(), "stale raw entry must be removed");
    assert!(fresh.exists(), "fresh raw entry must be retained");
    assert!(
        store.find_dir("retention-writer").is_ok(),
        "a concurrent current-day writer must remain readable"
    );
}

#[test]
fn packaged_install_status_verify_and_doctor_survive_a_fresh_process() {
    let tree = temporary_tree("keel-packaged-lifecycle");
    let bundle_root = tree.0.join("release-bundle");
    let claude_home = tree.0.join("keel-home");
    fs::create_dir_all(&bundle_root).expect("create release bundle root");
    stage_release_bundle(&bundle_root, &keel_binary());

    let bundle_root_argument = bundle_root.as_os_str();
    let claude_home_argument = claude_home.as_os_str();
    let install = run_cli(
        &keel_binary(),
        &[
            OsStr::new("install"),
            OsStr::new("--repo-root"),
            bundle_root_argument,
            OsStr::new("--claude-home"),
            claude_home_argument,
            OsStr::new("--without"),
            OsStr::new("opencode,codex,pi,cursor,cowork,commandcode,grok,omp,zcode,antigravity"),
        ],
        Some(&tree.0),
    );
    assert_success(&install, "packaged install");

    let executable_name = if cfg!(windows) { "keel.exe" } else { "keel" };
    let installed_binary = claude_home.join(executable_name);
    assert!(
        installed_binary.is_file(),
        "install must publish the packaged binary"
    );

    let metadata_path = claude_home.join("state").join("install-metadata.txt");
    let metadata = fs::read_to_string(&metadata_path).expect("read packaged install metadata");
    assert_eq!(metadata_value(&metadata, "source_kind"), Some("release"));
    assert_eq!(
        metadata_value(&metadata, "repository_slug"),
        Some("UntaDotMy/keel")
    );
    let cached_source = PathBuf::from(
        metadata_value(&metadata, "source_root").expect("packaged source cache path"),
    );
    assert!(
        cached_source.join("keel-release-manifest.json").is_file(),
        "packaged source must be cached for later verification"
    );
    assert!(cached_source.join("reviewer/SKILL.md").is_file());
    fs::remove_dir_all(&bundle_root).expect("remove transient release bundle");
    let cached_source_argument = cached_source.as_os_str();

    let lifecycle_arguments = |command: &'static str| {
        [
            OsStr::new(command),
            OsStr::new("--repo-root"),
            cached_source_argument,
            OsStr::new("--claude-home"),
            claude_home_argument,
        ]
    };
    let status = run_cli(
        &installed_binary,
        &lifecycle_arguments("status"),
        Some(&tree.0),
    );
    assert_success(&status, "packaged status after process restart");
    assert!(
        String::from_utf8_lossy(&status.stdout).contains("installed-source"),
        "status must use the cached packaged source after restart:\n{}",
        String::from_utf8_lossy(&status.stdout)
    );

    let verify = run_cli(
        &installed_binary,
        &lifecycle_arguments("verify"),
        Some(&tree.0),
    );
    assert_success(&verify, "packaged verify after process restart");
    assert!(
        String::from_utf8_lossy(&verify.stdout).contains("All Rust verification checks passed"),
        "verify must complete the packaged source checks:\n{}",
        String::from_utf8_lossy(&verify.stdout)
    );

    let doctor = run_cli(
        &installed_binary,
        &lifecycle_arguments("doctor"),
        Some(&tree.0),
    );
    assert_success(&doctor, "packaged doctor after process restart");
    assert!(
        String::from_utf8_lossy(&doctor.stdout).contains("Doctor:"),
        "doctor must execute the packaged health surface:\n{}",
        String::from_utf8_lossy(&doctor.stdout)
    );
}
