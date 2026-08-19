use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_WORKSPACE: AtomicU64 = AtomicU64::new(1);

pub fn create_workspace(files: &[String], gates: &[String]) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!(
        "anvil-ws-{}-{}",
        std::process::id(),
        NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for f in files.iter().chain(gates.iter()) {
        let p = Path::new(f);
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(dir.join(parent));
        }
        let src = PathBuf::from(f);
        if src.exists() {
            let _ = std::fs::copy(&src, dir.join(p));
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
    if src.is_file() {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::copy(src, dst).map_err(|error| error.to_string())?;
        return Ok(());
    }
    if !src.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dst).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(src).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let to = dst.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), to).map_err(|error| error.to_string())?;
        }
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
