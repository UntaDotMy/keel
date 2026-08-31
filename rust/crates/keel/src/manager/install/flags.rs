// Installer platform flags.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PlatformName {
    Opencode,
    Codex,
    Pi,
    Cursor,
    Cowork,
    Commandcode,
    Grok,
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
            "grok" | "grok-build" => Some(Self::Grok),
            _ => None,
        }
    }
}

#[derive(Debug, Default)]
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
    if overrides.force.contains(&PlatformName::Grok) {
        detected.grok = true;
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
    if overrides.skip.contains(&PlatformName::Grok) {
        detected.grok = false;
    }
    detected
}

/// True when `home` is a real user-level install root: the legacy `.claude`
/// home or the host-neutral `.keel` home. Every host-wiring gate keys off
/// this so adapters register for both layouts; non-standard roots (test temp
/// dirs, custom `--claude-home` overrides) keep wiring hermetic.
pub(crate) fn is_standard_home(home: &Path) -> bool {
    is_host_wiring_home_with_override(
        home,
        std::env::var_os("KEEL_HOME").map(PathBuf::from).as_deref(),
    )
}

fn is_host_wiring_home_with_override(home: &Path, override_home: Option<&Path>) -> bool {
    let conventional = home
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == ".claude" || name == crate::runtime::KEEL_HOME_DIRECTORY_NAME)
        .unwrap_or(false);
    conventional || override_home.is_some_and(|configured| configured == home)
}

pub(crate) fn host_user_home(keel_home: &Path) -> Option<PathBuf> {
    let conventional = keel_home
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == ".claude" || name == crate::runtime::KEEL_HOME_DIRECTORY_NAME);
    if conventional {
        return keel_home.parent().map(Path::to_path_buf);
    }
    crate::runtime::resolve_user_home().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_keel_home_is_eligible_only_when_it_matches_the_override() {
        let custom = Path::new("/srv/keel-data");
        assert!(!is_host_wiring_home_with_override(custom, None));
        assert!(is_host_wiring_home_with_override(custom, Some(custom)));
        assert!(!is_host_wiring_home_with_override(
            custom,
            Some(Path::new("/srv/other"))
        ));
    }
}
