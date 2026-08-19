//! Purpose: Release asset naming, URLs, cache paths, and bundle manifest helpers.
//! Caller: keel binary commands (`bootstrap-info`) and future installer flows.
//! Dependencies: std::path::{Path, PathBuf}; keel_platform::Target.
//! Main Functions: normalize_release_tag, archive_file_name, cache_directory, executable_path, installed_executable_path, release_download_url, bundle_manifest_path, build_bundle_manifest.
//! Side Effects: Pure value computation; no filesystem or network I/O. Rust-native release asset naming helpers.

use std::path::{Path, PathBuf};

use keel_platform::Target;

pub const DEFAULT_REPOSITORY_SLUG: &str = "UntaDotMy/keel";
pub const BUNDLE_MANIFEST_FILE_NAME: &str = "keel-release-manifest.json";
pub const PACKAGED_RELEASE_BUNDLE_KIND: &str = "packaged_release_bundle";

pub fn normalize_release_tag(build_version: &str) -> String {
    let trimmed_build_version = build_version.trim();
    let effective_build_version = if trimmed_build_version.is_empty() {
        "dev"
    } else {
        trimmed_build_version
    };

    if effective_build_version.starts_with('v') {
        return effective_build_version.to_string();
    }

    if !starts_with_ascii_digit(effective_build_version) {
        return effective_build_version.to_string();
    }

    format!("v{effective_build_version}")
}

fn starts_with_ascii_digit(value: &str) -> bool {
    match value.as_bytes().first() {
        Some(first_byte) => first_byte.is_ascii_digit(),
        None => false,
    }
}

fn has_semantic_version_tag_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2 && bytes[0] == b'v' && bytes[1].is_ascii_digit()
}

fn build_version_for_release_assets(build_version: &str) -> String {
    let normalized_release_tag = normalize_release_tag(build_version);
    if has_semantic_version_tag_prefix(&normalized_release_tag) {
        normalized_release_tag[1..].to_string()
    } else {
        normalized_release_tag
    }
}

pub fn archive_file_name(build_version: &str, target: &Target) -> String {
    let archive_extension = if target.operating_system == "windows" {
        ".zip"
    } else {
        ".tar.gz"
    };
    format!(
        "keel_{}_{}_{}{archive_extension}",
        build_version_for_release_assets(build_version),
        target.operating_system,
        target.architecture,
    )
}

pub fn cache_directory(
    claude_home_directory: &str,
    build_version: &str,
    target: &Target,
) -> PathBuf {
    let mut cache_path = PathBuf::from(claude_home_directory);
    cache_path.push("cache");
    cache_path.push("update");
    cache_path.push(normalize_release_tag(build_version));
    cache_path.push(target.directory_name());
    cache_path
}

pub fn executable_path(
    claude_home_directory: &str,
    build_version: &str,
    target: &Target,
) -> PathBuf {
    let mut executable_path = cache_directory(claude_home_directory, build_version, target);
    executable_path.push(target.executable_name());
    executable_path
}

pub fn installed_executable_path(claude_home_directory: &str, target: &Target) -> PathBuf {
    let mut installed_executable_path = PathBuf::from(claude_home_directory);
    installed_executable_path.push(target.executable_name());
    installed_executable_path
}

pub fn release_download_url(repository_slug: &str, build_version: &str, target: &Target) -> String {
    let trimmed_repository_slug = repository_slug.trim();
    let effective_repository_slug = if trimmed_repository_slug.is_empty() {
        DEFAULT_REPOSITORY_SLUG
    } else {
        trimmed_repository_slug
    };
    let release_tag = normalize_release_tag(build_version);
    format!(
        "https://github.com/{effective_repository_slug}/releases/download/{release_tag}/{}",
        archive_file_name(build_version, target),
    )
}

pub fn bundle_manifest_path(repository_root: &Path) -> PathBuf {
    repository_root.join(BUNDLE_MANIFEST_FILE_NAME)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleManifest {
    pub package_kind: String,
    pub repository_slug: String,
    pub release_tag: String,
    pub build_version: String,
    pub archive_file_name: String,
}

pub fn build_bundle_manifest(
    repository_slug: &str,
    build_version: &str,
    target: &Target,
) -> BundleManifest {
    let trimmed_repository_slug = repository_slug.trim();
    let effective_repository_slug = if trimmed_repository_slug.is_empty() {
        DEFAULT_REPOSITORY_SLUG.to_string()
    } else {
        trimmed_repository_slug.to_string()
    };
    BundleManifest {
        package_kind: PACKAGED_RELEASE_BUNDLE_KIND.to_string(),
        repository_slug: effective_repository_slug,
        release_tag: normalize_release_tag(build_version),
        build_version: build_version_for_release_assets(build_version),
        archive_file_name: archive_file_name(build_version, target),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux_amd64() -> Target {
        Target {
            operating_system: "linux".into(),
            architecture: "amd64".into(),
        }
    }

    fn linux_arm64() -> Target {
        Target {
            operating_system: "linux".into(),
            architecture: "arm64".into(),
        }
    }

    fn windows_amd64() -> Target {
        Target {
            operating_system: "windows".into(),
            architecture: "amd64".into(),
        }
    }

    fn darwin_arm64() -> Target {
        Target {
            operating_system: "darwin".into(),
            architecture: "arm64".into(),
        }
    }

    #[test]
    fn archive_file_name_matches_target_packaging() {
        assert_eq!(
            archive_file_name("2026.03.14.0", &darwin_arm64()),
            "keel_2026.03.14.0_darwin_arm64.tar.gz"
        );
        assert_eq!(
            archive_file_name("2026.03.14.0", &windows_amd64()),
            "keel_2026.03.14.0_windows_amd64.zip"
        );
        assert_eq!(
            archive_file_name("bootstrap-sample-release", &linux_amd64()),
            "keel_bootstrap-sample-release_linux_amd64.tar.gz"
        );
    }

    #[test]
    fn cache_directory_uses_claude_home_and_normalized_tag() {
        let cached = cache_directory("/tmp/claude-home", "2026.03.14.0", &linux_amd64());
        assert_eq!(
            cached,
            PathBuf::from("/tmp/claude-home/cache/update/v2026.03.14.0/linux-amd64")
        );

        let bootstrap_cached = cache_directory(
            "/tmp/claude-home",
            "bootstrap-sample-release",
            &linux_amd64(),
        );
        assert_eq!(
            bootstrap_cached,
            PathBuf::from("/tmp/claude-home/cache/update/bootstrap-sample-release/linux-amd64")
        );
    }

    #[test]
    fn release_download_url_uses_default_repository_when_unset() {
        assert_eq!(
            release_download_url("", "2026.03.14.0", &linux_arm64()),
            "https://github.com/UntaDotMy/keel/releases/download/v2026.03.14.0/keel_2026.03.14.0_linux_arm64.tar.gz"
        );
        assert_eq!(
            release_download_url("", "bootstrap-sample-release", &linux_arm64()),
            "https://github.com/UntaDotMy/keel/releases/download/bootstrap-sample-release/keel_bootstrap-sample-release_linux_arm64.tar.gz"
        );
    }

    #[test]
    fn installed_executable_path_uses_claude_home_root() {
        assert_eq!(
            installed_executable_path("/tmp/claude-home", &linux_amd64()),
            PathBuf::from("/tmp/claude-home/keel")
        );
        assert_eq!(
            installed_executable_path("C:/Users/example/.claude", &windows_amd64()),
            PathBuf::from("C:/Users/example/.claude/keel.exe")
        );
    }

    #[test]
    fn build_bundle_manifest_uses_normalized_release_metadata() {
        let manifest = build_bundle_manifest("", "2026.03.14.0", &linux_arm64());
        assert_eq!(manifest.package_kind, PACKAGED_RELEASE_BUNDLE_KIND);
        assert_eq!(manifest.repository_slug, DEFAULT_REPOSITORY_SLUG);
        assert_eq!(manifest.release_tag, "v2026.03.14.0");
        assert_eq!(manifest.build_version, "2026.03.14.0");
        assert_eq!(
            manifest.archive_file_name,
            "keel_2026.03.14.0_linux_arm64.tar.gz"
        );
    }

    #[test]
    fn normalize_release_tag_handles_empty_and_prefixed_inputs() {
        assert_eq!(normalize_release_tag(""), "dev");
        assert_eq!(normalize_release_tag("  "), "dev");
        assert_eq!(normalize_release_tag("v1.2.3"), "v1.2.3");
        assert_eq!(normalize_release_tag("1.2.3"), "v1.2.3");
        assert_eq!(normalize_release_tag("nightly"), "nightly");
    }
}
