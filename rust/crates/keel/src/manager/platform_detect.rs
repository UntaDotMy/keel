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
    pub cowork: bool,
    pub commandcode: bool,
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
            // Cowork (Claude Desktop) detection:
            // Claude Desktop Cowork uses the same ~/.claude directory as Claude Code,
            // but stores a specific app identifier file to distinguish itself.
            // Detection strategy:
            // 1. Check for .claude directory with Cowork-specific config
            // 2. Check for CLAUDE_DESKTOP_HOME env var
            // 3. Check for Claude Desktop binary on PATH
            cowork: self.has_config_dir(".claude-desktop")
                || self.has_binary("claude-desktop")
                || std::env::var("CLAUDE_DESKTOP_HOME").is_ok()
                || self.detect_cowork_via_claude_dir(),
            // Command Code: config dir ~/.commandcode or the cmdc binary on PATH.
            commandcode: self.has_config_dir(".commandcode") || self.has_binary("cmdc"),
        }
    }

    /// Detect Cowork by checking for Claude Desktop-specific markers in ~/.claude
    fn detect_cowork_via_claude_dir(&self) -> bool {
        let claude_dir = self.home.join(".claude");
        if !claude_dir.is_dir() {
            return false;
        }

        // Check for Cowork-specific files that distinguish it from Claude Code CLI
        // These files are created by the Cowork app and not by the CLI
        let cowork_markers = [
            "cowork.session",    // Cowork session marker
            "cowork.lock",       // Cowork lock file
            ".cowork.installed", // Cowork install marker
        ];

        for marker in &cowork_markers {
            if claude_dir.join(marker).exists() {
                return true;
            }
        }

        // Also check if there's a plugins directory with Cowork-specific plugins
        let plugins_dir = claude_dir.join("plugins");
        if plugins_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&plugins_dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    // Look for Cowork-specific plugins
                    if name_str.contains("cowork") || name_str.contains("claude-desktop") {
                        return true;
                    }
                }
            }
        }

        false
    }

    fn has_config_dir(&self, relative: &str) -> bool {
        self.home.join(relative).is_dir()
    }

    fn has_binary(&self, name: &str) -> bool {
        which::which(name).is_ok()
    }
}
