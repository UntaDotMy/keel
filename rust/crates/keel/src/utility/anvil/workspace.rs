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
    let result = (|| {
        let _ = gates;
        for raw in files {
            let (relative, source) = source_file(&root, raw)?;
            if source.is_file() {
                if let Some(parent) = relative.parent() {
                    std::fs::create_dir_all(dir.join(parent)).map_err(|error| error.to_string())?;
                }
                std::fs::copy(&source, dir.join(&relative)).map_err(|error| error.to_string())?;
            }
        }
        Ok::<(), String>(())
    })();
    if let Err(error) = result {
        let _ = remove_workspace(&dir);
        return Err(error);
    }
    Ok(TemporaryWorkspace { path: dir })
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

pub fn paginated_read(path: &Path, offset: usize, limit: usize) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let lines: Vec<&str> = text.lines().collect();
    let end = (offset + limit).min(lines.len());
    let slice = if offset < lines.len() {
        &lines[offset..end]
    } else {
        &[] as &[&str]
    };
    let mut out = String::new();
    for (i, line) in slice.iter().enumerate() {
        out.push_str(&format!("{:4}: {}\n", offset + i + 1, line));
    }
    Ok(out)
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
    fn failed_workspace_creation_removes_its_temporary_directory() {
        let root = test_root();
        let sequence = NEXT_WORKSPACE.load(Ordering::Relaxed);
        let expected =
            std::env::temp_dir().join(format!("anvil-ws-{}-{sequence}", std::process::id()));
        let error = create_temporary_workspace(root.path(), &["missing.txt".to_string()], &[])
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
