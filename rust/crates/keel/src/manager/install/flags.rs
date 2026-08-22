// Installer platform flags.
use std::collections::BTreeSet;
use std::path::Path;
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PlatformName {
    Opencode,
    Codex,
    Pi,
    Cursor,
    Cowork,
    Commandcode,
}

impl PlatformName {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "opencode" => Some(Self::Opencode),
            "codex" => Some(Self::Codex),
            "pi" => Some(Self::Pi),
            "cursor" => Some(Self::Cursor),
            "cowork" | "desktop" => Some(Self::Cowork),
            "commandcode" | "cmdc" | "command-code" => Some(Self::Commandcode),
            _ => None,
        }
    }
}

#[derive(Default)]
pub struct InstallOverrides {
    pub force: BTreeSet<PlatformName>,
    pub skip: BTreeSet<PlatformName>,
}

pub(crate) fn parse_overrides(with: &str, without: &str) -> InstallOverrides {
    let mut overrides = InstallOverrides::default();
    for name in with.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(p) = PlatformName::parse(name) {
            overrides.force.insert(p);
        }
    }
    for name in without.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(p) = PlatformName::parse(name) {
            overrides.skip.insert(p);
        }
    }
    overrides
}

pub(crate) fn apply_overrides(
    mut detected: super::super::platform_detect::DetectedPlatforms,
    overrides: &InstallOverrides,
) -> super::super::platform_detect::DetectedPlatforms {
    if overrides.force.contains(&PlatformName::Opencode) {
        detected.opencode = true;
    }
    if overrides.force.contains(&PlatformName::Codex) {
        detected.codex = true;
    }
    if overrides.force.contains(&PlatformName::Pi) {
        detected.pi = true;
    }
    if overrides.force.contains(&PlatformName::Cursor) {
        detected.cursor = true;
    }
    if overrides.force.contains(&PlatformName::Cowork) {
        detected.cowork = true;
    }
    if overrides.force.contains(&PlatformName::Commandcode) {
        detected.commandcode = true;
    }
    if overrides.skip.contains(&PlatformName::Opencode) {
        detected.opencode = false;
    }
    if overrides.skip.contains(&PlatformName::Codex) {
        detected.codex = false;
    }
    if overrides.skip.contains(&PlatformName::Pi) {
        detected.pi = false;
    }
    if overrides.skip.contains(&PlatformName::Cursor) {
        detected.cursor = false;
    }
    if overrides.skip.contains(&PlatformName::Cowork) {
        detected.cowork = false;
    }
    if overrides.skip.contains(&PlatformName::Commandcode) {
        detected.commandcode = false;
    }
    detected
}

/// True when `home` is a real user-level install root: the legacy `.claude`
/// home or the host-neutral `.keel` home. Every host-wiring gate keys off
/// this so adapters register for both layouts; non-standard roots (test temp
/// dirs, custom `--claude-home` overrides) keep wiring hermetic.
pub(crate) fn is_standard_home(home: &Path) -> bool {
    home.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == ".claude" || name == crate::runtime::KEEL_HOME_DIRECTORY_NAME)
        .unwrap_or(false)
}
