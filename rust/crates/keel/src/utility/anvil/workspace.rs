use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_WORKSPACE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub struct TemporaryWorkspace {
    path: PathBuf,
}

impl TemporaryWorkspace {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryWorkspace {
    fn drop(&mut self) {
        for _ in 0..5 {
            if remove_workspace(&self.path).is_ok() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

fn safe_relative_path(raw: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(raw);
    if raw.trim().is_empty() {
        return Err("anvil workspace path is empty".to_string());
    }
    if candidate.is_absolute() {
        return Err(format!("anvil workspace path must be relative: {raw}"));
    }
    for component in candidate.components() {
        if matches!(
            component,
            Component::Prefix(_) | Component::RootDir | Component::ParentDir
        ) {
            return Err(format!("anvil workspace path escapes its root: {raw}"));
        }
    }
    Ok(candidate.to_path_buf())
}

fn canonical_workspace_root(root: &Path) -> Result<PathBuf, String> {
    let canonical = root
        .canonicalize()
        .map_err(|error| format!("anvil workspace root: {error}"))?;
    if !canonical.is_dir() {
        return Err(format!(
            "anvil workspace root is not a directory: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

fn source_file(root: &Path, raw: &str) -> Result<(PathBuf, PathBuf), String> {
    let relative = safe_relative_path(raw)?;
    let source = root.join(&relative);
    let metadata = std::fs::symlink_metadata(&source)
        .map_err(|error| format!("anvil workspace source {raw}: {error}"))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("anvil workspace source cannot be a symlink: {raw}"));
    }
    let canonical = source
        .canonicalize()
        .map_err(|error| format!("anvil workspace source {raw}: {error}"))?;
    if !canonical.starts_with(root) {
        return Err(format!("anvil workspace source escapes its root: {raw}"));
    }
    Ok((relative, canonical))
}

pub fn validate_workspace_files(workspace_root: &Path, files: &[String]) -> Result<(), String> {
    let root = canonical_workspace_root(workspace_root)?;
    for raw in files {
        let (_, source) = source_file(&root, raw)?;
        if !source.is_file() {
            return Err(format!("anvil workspace source is not a file: {raw}"));
        }
    }
    Ok(())
}

pub fn cleanup_stale_workspaces(minimum_age: std::time::Duration) -> (usize, Vec<String>) {
    let mut removed = 0usize;
    let mut errors = Vec::new();
    let entries = match std::fs::read_dir(std::env::temp_dir()) {
        Ok(entries) => entries,
        Err(error) => return (0, vec![format!("list temporary directory: {error}")]),
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_owned_workspace_name(&name) {
            continue;
        }
        let path = entry.path();
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => metadata,
            _ => continue,
        };
        let stale = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= minimum_age);
        if !stale {
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => removed += 1,
            Err(error) => errors.push(format!("remove {}: {error}", path.display())),
        }
    }
    (removed, errors)
}

fn is_owned_workspace_name(name: &str) -> bool {
    let parts: Vec<&str> = name.split('-').collect();
    parts.len() == 4
        && parts[0] == "anvil"
        && parts[1] == "ws"
        && !parts[2].is_empty()
        && parts[2].bytes().all(|byte| byte.is_ascii_digit())
        && !parts[3].is_empty()
        && parts[3].bytes().all(|byte| byte.is_ascii_digit())
}

pub fn create_temporary_workspace(
    workspace_root: &Path,
    files: &[String],
    gates: &[String],
) -> Result<TemporaryWorkspace, String> {
    let root = canonical_workspace_root(workspace_root)?;
    validate_gate_context(files, gates)?;
    let dir = loop {
        let candidate = std::env::temp_dir().join(format!(
            "anvil-ws-{}-{}",
            std::process::id(),
            NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::create_dir(&candidate) {
            Ok(()) => break candidate,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    };
    populate_temporary_workspace(&root, TemporaryWorkspace { path: dir }, files)
}

/// Ensure a selected-file workspace contains the manifest/context required by
/// its deterministic gates before a host builder is started. Anvil keeps the
/// writable workspace intentionally narrow, so silently launching `cargo test`
/// without a manifest only produces a late, misleading gate failure.
fn validate_gate_context(files: &[String], gates: &[String]) -> Result<(), String> {
    let selected = files
        .iter()
        .filter_map(|file| safe_relative_path(file).ok())
        .map(|file| {
            file.to_string_lossy()
                .replace('\\', "/")
                .to_ascii_lowercase()
        })
        .collect::<std::collections::BTreeSet<_>>();

    let mut required = Vec::new();
    for gate in gates {
        let command = gate.to_ascii_lowercase();
        if command
            .split_whitespace()
            .any(|token| token == "cargo" || token.ends_with("/cargo"))
        {
            required.push("Cargo.toml");
            if command.contains("--locked") || command.contains("--frozen") {
                required.push("Cargo.lock");
            }
        }
        if command
            .split_whitespace()
            .any(|token| token == "npm" || token.ends_with("/npm"))
        {
            required.push("package.json");
        }
        if command
            .split_whitespace()
            .any(|token| token == "go" || token.ends_with("/go"))
        {
            required.push("go.mod");
        }
        if command.split_whitespace().any(|token| token == "pytest")
            && !selected.contains("pyproject.toml")
            && !selected.contains("setup.cfg")
            && !selected.contains("setup.py")
        {
            return Err(format!(
                "anvil gate context is incomplete: `{gate}` needs pyproject.toml, setup.cfg, or setup.py in the selected files"
            ));
        }
    }
    for manifest in required {
        if !selected.contains(&manifest.to_ascii_lowercase()) {
            return Err(format!(
                "anvil gate context is incomplete: gate `{}` requires {manifest} in the selected files",
                gates.iter().find(|gate| gate.to_ascii_lowercase().contains(&manifest.to_ascii_lowercase())).map(String::as_str).unwrap_or("selected gates")
            ));
        }
    }
    Ok(())
}

fn populate_temporary_workspace(
    root: &Path,
    workspace: TemporaryWorkspace,
    files: &[String],
) -> Result<TemporaryWorkspace, String> {
    for raw in files {
        let (relative, source) = source_file(root, raw)?;
        if source.is_file() {
            if let Some(parent) = relative.parent() {
                std::fs::create_dir_all(workspace.path().join(parent))
                    .map_err(|error| error.to_string())?;
            }
            std::fs::copy(&source, workspace.path().join(&relative))
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(workspace)
}

fn remove_workspace(dir: &Path) -> Result<(), String> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(src).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err(format!("anvil copy refuses symlink: {}", src.display()));
    }
    if metadata.is_file() {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::copy(src, dst).map_err(|error| error.to_string())?;
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dst).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(src).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let to = dst.join(entry.file_name());
        copy_tree(&entry.path(), &to)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            for _ in 0..5 {
                match std::fs::remove_dir_all(&self.0) {
                    Ok(()) => return,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
                }
            }
        }
    }

    fn test_root() -> TempRoot {
        let root = std::env::temp_dir().join(format!(
            "anvil-workspace-test-{}-{}",
            std::process::id(),
            NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).expect("root");
        TempRoot(root)
    }

    #[test]
    fn rejects_absolute_and_parent_paths() {
        assert!(safe_relative_path("../outside").is_err());
        assert!(safe_relative_path("/outside").is_err());
        let absolute = if cfg!(windows) {
            r"C:\outside"
        } else {
            "/outside"
        };
        assert!(safe_relative_path(absolute).is_err());
        assert!(safe_relative_path("src/main.rs").is_ok());
    }

    #[test]
    fn copies_files_from_selected_workspace_root() {
        let root = test_root();
        std::fs::create_dir_all(root.path().join("src")).expect("src");
        std::fs::write(root.path().join("src/main.rs"), "fn main() {}").expect("source");
        let workspace = create_temporary_workspace(root.path(), &["src/main.rs".to_string()], &[])
            .expect("workspace");
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("src/main.rs")).expect("copied"),
            "fn main() {}"
        );
        let path = workspace.path().to_path_buf();
        drop(workspace);
        assert!(!path.exists());
    }

    #[test]
    fn rejects_build_gate_without_selected_manifest() {
        let root = test_root();
        std::fs::create_dir_all(root.path().join("src")).expect("src");
        std::fs::write(root.path().join("src/main.rs"), "fn main() {}").expect("source");
        let error = create_temporary_workspace(
            root.path(),
            &["src/main.rs".to_string()],
            &["cargo test --workspace --locked".to_string()],
        )
        .expect_err("cargo gate must require selected manifests");
        assert!(error.contains("Cargo.toml"), "actionable error: {error}");
    }

    #[test]
    fn accepts_cargo_gate_when_selected_manifests_are_present() {
        let root = test_root();
        std::fs::create_dir_all(root.path().join("src")).expect("src");
        std::fs::write(root.path().join("src/main.rs"), "fn main() {}").expect("source");
        std::fs::write(
            root.path().join("Cargo.toml"),
            "[package]\nname='x'\nversion='0.1.0'\nedition='2021'\n",
        )
        .expect("manifest");
        std::fs::write(root.path().join("Cargo.lock"), "# lockfile v4\n").expect("lockfile");
        let workspace = create_temporary_workspace(
            root.path(),
            &[
                "src/main.rs".to_string(),
                "Cargo.toml".to_string(),
                "Cargo.lock".to_string(),
            ],
            &["cargo test --workspace --locked".to_string()],
        )
        .expect("selected build context");
        assert!(workspace.path().join("Cargo.toml").is_file());
        assert!(workspace.path().join("Cargo.lock").is_file());
    }

    #[test]
    fn failed_workspace_creation_removes_its_temporary_directory() {
        let root = test_root();
        let expected = root.path().join("failed-workspace");
        std::fs::create_dir(&expected).expect("workspace");
        let workspace = TemporaryWorkspace {
            path: expected.clone(),
        };
        let error =
            populate_temporary_workspace(root.path(), workspace, &["missing.txt".to_string()])
                .expect_err("missing source must fail");
        assert!(error.contains("missing.txt"));
        assert!(
            !expected.exists(),
            "failed create leaked {}",
            expected.display()
        );
    }

    #[test]
    fn stale_cleanup_only_recognizes_strictly_owned_workspace_names() {
        assert!(is_owned_workspace_name("anvil-ws-123-456"));
        assert!(!is_owned_workspace_name("anvil-ws-user-data-456"));
        assert!(!is_owned_workspace_name("anvil-ws-123-456-backup"));
        assert!(!is_owned_workspace_name("anvil-ws--456"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_sources() {
        let root = test_root();
        let outside = root
            .path()
            .parent()
            .expect("temp parent")
            .join(format!("anvil-outside-{}", std::process::id()));
        std::fs::write(&outside, "outside").expect("outside");
        std::os::unix::fs::symlink(&outside, root.path().join("linked")).expect("symlink");
        assert!(create_temporary_workspace(root.path(), &["linked".to_string()], &[]).is_err());
        std::fs::remove_file(root.path().join("linked")).expect("link");
        std::fs::remove_file(outside).expect("outside");
    }
}
