use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_WORKSPACE: AtomicU64 = AtomicU64::new(1);

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

pub fn create_workspace(
    workspace_root: &Path,
    files: &[String],
    gates: &[String],
) -> Result<PathBuf, String> {
    let root = canonical_workspace_root(workspace_root)?;
    let dir = std::env::temp_dir().join(format!(
        "anvil-ws-{}-{}",
        std::process::id(),
        NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
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
    Ok(dir)
}

pub fn remove_workspace(dir: &Path) -> Result<(), String> {
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

    fn test_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "anvil-workspace-test-{}",
            NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).expect("root");
        root
    }

    #[test]
    fn rejects_absolute_and_parent_paths() {
        assert!(safe_relative_path("../outside").is_err());
        assert!(safe_relative_path("/outside").is_err());
        assert!(safe_relative_path("C:\\outside").is_err());
        assert!(safe_relative_path("src/main.rs").is_ok());
    }

    #[test]
    fn copies_files_from_selected_workspace_root() {
        let root = test_root();
        std::fs::create_dir_all(root.join("src")).expect("src");
        std::fs::write(root.join("src/main.rs"), "fn main() {}").expect("source");
        let workspace =
            create_workspace(&root, &["src/main.rs".to_string()], &[]).expect("workspace");
        assert_eq!(
            std::fs::read_to_string(workspace.join("src/main.rs")).expect("copied"),
            "fn main() {}"
        );
        remove_workspace(&workspace).expect("remove workspace");
        std::fs::remove_dir_all(root).expect("remove root");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_sources() {
        let root = test_root();
        let outside = root.parent().expect("temp parent").join("anvil-outside");
        std::fs::write(&outside, "outside").expect("outside");
        std::os::unix::fs::symlink(&outside, root.join("linked")).expect("symlink");
        assert!(create_workspace(&root, &["linked".to_string()], &[]).is_err());
        std::fs::remove_file(root.join("linked")).expect("link");
        std::fs::remove_file(outside).expect("outside");
        std::fs::remove_dir_all(root).expect("remove root");
    }
}
