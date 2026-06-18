//! Purpose: Target detection and normalization for the native keel CLI.
//! Caller: keel binary commands (`version`, `platform`, `bootstrap-info`) and downstream crates that need OS/arch metadata.
//! Dependencies: std::fmt; cfg(target_os) and cfg(target_arch) compile-time selectors.
//! Main Functions: detect_current_target, normalize_target, Target::directory_name, Target::executable_name.
//! Side Effects: None; the module is a pure value type. Rust-native platform target helpers.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub operating_system: String,
    pub architecture: String,
}

impl Target {
    pub fn directory_name(&self) -> String {
        format!("{}-{}", self.operating_system, self.architecture)
    }

    pub fn executable_name(&self) -> &'static str {
        if self.operating_system == "windows" {
            "keel.exe"
        } else {
            "keel"
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizeTargetError(pub String);

impl fmt::Display for NormalizeTargetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for NormalizeTargetError {}

pub fn detect_current_target() -> Result<Target, NormalizeTargetError> {
    let runtime_operating_system = current_runtime_operating_system();
    let runtime_architecture = current_runtime_architecture();
    normalize_target(&runtime_operating_system, &runtime_architecture)
}

pub fn normalize_target(
    runtime_operating_system: &str,
    runtime_architecture: &str,
) -> Result<Target, NormalizeTargetError> {
    let normalized_operating_system = normalize_operating_system(runtime_operating_system)?;
    let normalized_architecture = normalize_architecture(runtime_architecture)?;
    Ok(Target {
        operating_system: normalized_operating_system,
        architecture: normalized_architecture,
    })
}

fn normalize_operating_system(
    runtime_operating_system: &str,
) -> Result<String, NormalizeTargetError> {
    match runtime_operating_system {
        "darwin" | "linux" | "windows" => Ok(runtime_operating_system.to_string()),
        _ => Err(NormalizeTargetError(format!(
            "unsupported operating system: {runtime_operating_system}"
        ))),
    }
}

fn normalize_architecture(runtime_architecture: &str) -> Result<String, NormalizeTargetError> {
    match runtime_architecture {
        "amd64" | "arm64" => Ok(runtime_architecture.to_string()),
        "x86_64" => Ok("amd64".to_string()),
        "aarch64" => Ok("arm64".to_string()),
        _ => Err(NormalizeTargetError(format!(
            "unsupported architecture: {runtime_architecture}"
        ))),
    }
}

fn current_runtime_operating_system() -> String {
    match std::env::consts::OS {
        "macos" => "darwin".to_string(),
        other => other.to_string(),
    }
}

fn current_runtime_architecture() -> String {
    std::env::consts::ARCH.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_target_maps_supported_aliases() {
        let target = normalize_target("darwin", "x86_64").expect("normalize_target should succeed");
        assert_eq!(target.operating_system, "darwin");
        assert_eq!(target.architecture, "amd64");
    }

    #[test]
    fn normalize_target_rejects_unsupported_values() {
        assert!(normalize_target("plan9", "amd64").is_err());
        assert!(normalize_target("linux", "mips").is_err());
    }

    #[test]
    fn executable_name_matches_platform() {
        let windows_target = Target {
            operating_system: "windows".into(),
            architecture: "amd64".into(),
        };
        assert_eq!(windows_target.executable_name(), "keel.exe");

        let linux_target = Target {
            operating_system: "linux".into(),
            architecture: "amd64".into(),
        };
        assert_eq!(linux_target.executable_name(), "keel");
    }

    #[test]
    fn directory_name_joins_with_dash() {
        let target = Target {
            operating_system: "linux".into(),
            architecture: "amd64".into(),
        };
        assert_eq!(target.directory_name(), "linux-amd64");
    }
}
