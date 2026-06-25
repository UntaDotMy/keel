//! Platform detection: discovers which AI CLI tools are installed so
//! `keel install` wires adapters only for present platforms.
//!
//! Detection uses two signals:
//! 1. Config-directory presence (e.g. `~/.config/opencode/`, `~/.codex/`)
//! 2. Binary-on-PATH via the `which` crate
//!
//! Cursor is never auto-detected — there is no reliable cross-platform
//! signal for Cursor IDE installation. Use `--with cursor` to force it.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct DetectedPlatforms {
    pub opencode: bool,
    pub codex: bool,
    pub pi: bool,
    pub cursor: bool,
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
            cursor: false,
        }
    }

    fn has_config_dir(&self, relative: &str) -> bool {
        self.home.join(relative).is_dir()
    }

    fn has_binary(&self, name: &str) -> bool {
        which::which(name).is_ok()
    }
}
