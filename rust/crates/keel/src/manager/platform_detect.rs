//! Platform detection: discovers which AI CLI tools are installed so
//! `keel install` wires adapters only for present platforms.
//!
//! Detection uses two signals:
//! 1. Config-directory presence (e.g. `~/.config/opencode/`, `~/.codex/`)
//! 2. Binary-on-PATH via the `which` crate
//!
//! Cursor auto-detects `~/.cursor/`. Use `--with cursor` to force wiring when
//! that directory is absent. Do not create Cursor files unless detected or forced.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct DetectedPlatforms {
    pub opencode: bool,
    pub codex: bool,
    pub pi: bool,
    pub cursor: bool,
    pub cowork: bool,
    pub commandcode: bool,
    pub grok: bool,
}

pub struct PlatformDetector {
    home: PathBuf,
}

impl PlatformDetector {
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.to_path_buf(),
        }
    }

    pub fn detect(&self) -> DetectedPlatforms {
        DetectedPlatforms {
            opencode: self.has_config_dir(".config/opencode") || self.has_binary("opencode"),
            codex: self.has_config_dir(".codex") || self.has_binary("codex"),
            pi: self.has_config_dir(".pi/agent") || self.has_binary("pi"),
            cursor: self.has_config_dir(".cursor"),
            cowork: {
                let config = super::install::claude_desktop_config_path(&self.home);
                config.is_file()
                    || config.parent().is_some_and(Path::is_dir)
                    || self.has_binary("claude-desktop")
            },
            // Command Code: config dir ~/.commandcode or the cmdc binary on PATH.
            commandcode: self.has_config_dir(".commandcode") || self.has_binary("cmdc"),
            grok: self.grok_home().is_dir() || self.has_binary("grok"),
        }
    }

    fn grok_home(&self) -> PathBuf {
        super::install::grok_config_home(&self.home)
    }

    fn has_config_dir(&self, relative: &str) -> bool {
        self.home.join(relative).is_dir()
    }

    fn has_binary(&self, name: &str) -> bool {
        which::which(name).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_detects_existing_cursor_dir() {
        let root = std::env::temp_dir().join(format!(
            "keel-cursor-detect-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(root.join(".cursor")).unwrap();
        let detected = PlatformDetector::new(&root).detect();
        assert!(detected.cursor);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cursor_stays_off_without_cursor_dir() {
        let root = std::env::temp_dir().join(format!(
            "keel-cursor-absent-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let detected = PlatformDetector::new(&root).detect();
        assert!(!detected.cursor);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cowork_detects_the_native_desktop_config() {
        let root = std::env::temp_dir().join(format!(
            "keel-cowork-detect-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let config = super::super::install::claude_desktop_config_path(&root);
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, "{}").unwrap();
        assert!(PlatformDetector::new(&root).detect().cowork);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cowork_does_not_trust_invented_claude_cli_markers() {
        let root = std::env::temp_dir().join(format!(
            "keel-cowork-fake-marker-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let claude = root.join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(claude.join("cowork.session"), "").unwrap();
        assert!(!PlatformDetector::new(&root).detect().cowork);
        let _ = std::fs::remove_dir_all(&root);
    }
}
