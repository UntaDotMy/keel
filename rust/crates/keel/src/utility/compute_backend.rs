//! Purpose: Pick the compute backend for the offline decision trainer: the local GPU when a vendor tool reports one, else the CPU.
//! Caller: manager::doctor (compute line) and the offline trainer to come.
//! Dependencies: std::process.
//! Main Functions: detect_compute_profile, ComputeProfile, ComputeBackend.
//! Side Effects: Runs vendor probe commands; reads nothing else.

use std::process::Command;

/// Backend the offline decision trainer should use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputeBackend {
    /// Portable fallback: always available, never fails closed.
    Cpu,
    /// A vendor tool named a device, so the trainer may run accelerated.
    Gpu,
}

impl ComputeBackend {
    /// Lowercase label shared by doctor output and logs.
    pub fn label(self) -> &'static str {
        match self {
            ComputeBackend::Cpu => "cpu",
            ComputeBackend::Gpu => "gpu",
        }
    }
}

/// The chosen backend plus the evidence for it: the device name when a GPU was
/// found, or the reason the CPU is in use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputeProfile {
    pub backend: ComputeBackend,
    pub detail: String,
}

/// Vendor probes in priority order. Adding a vendor is one entry here: a probe
/// contributes a device only when its first non-empty output line names one.
const GPU_PROBES: &[(&str, &[&str])] =
    &[("nvidia-smi", &["--query-gpu=name", "--format=csv,noheader"])];

/// Detect the accelerator available right now. Fail-closed: a missing vendor
/// tool, a failed probe, or empty output all mean the CPU backend, and only a
/// named device reports the GPU backend.
pub fn detect_compute_profile() -> ComputeProfile {
    detect_compute_profile_with(&run_probe)
}

fn detect_compute_profile_with(run: &dyn Fn(&str, &[&str]) -> Option<String>) -> ComputeProfile {
    for (tool, args) in GPU_PROBES {
        let Some(output) = run(tool, args) else {
            continue;
        };
        if let Some(device) = first_device_name(&output) {
            return ComputeProfile {
                backend: ComputeBackend::Gpu,
                detail: format!("{device} via {tool}"),
            };
        }
    }
    ComputeProfile {
        backend: ComputeBackend::Cpu,
        detail: "no vendor GPU tool reported a device".to_string(),
    }
}

fn run_probe(tool: &str, args: &[&str]) -> Option<String> {
    // why: a missing vendor tool is the normal no-GPU case, not an error.
    let output = Command::new(tool).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

/// First non-empty line of a probe's stdout, trimmed and bounded for display.
fn first_device_name(output: &str) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(80).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_named_device_selects_the_gpu_backend() {
        let profile =
            detect_compute_profile_with(&|_tool, _args| Some("NVIDIA GeForce RTX 4070\n".into()));
        assert_eq!(profile.backend, ComputeBackend::Gpu);
        assert!(
            profile.detail.contains("RTX 4070"),
            "detail: {}",
            profile.detail
        );
    }

    #[test]
    fn a_failed_or_empty_probe_falls_back_to_cpu() {
        let failed = detect_compute_profile_with(&|_tool, _args| None);
        assert_eq!(failed.backend, ComputeBackend::Cpu);
        let blank = detect_compute_profile_with(&|_tool, _args| Some("\n   \n".into()));
        assert_eq!(blank.backend, ComputeBackend::Cpu);
    }

    /// Live probe: passes on any machine, and the printed profile is the
    /// evidence on hardware that has a vendor GPU (run with --nocapture).
    #[test]
    fn real_probe_reports_a_profile() {
        let profile = detect_compute_profile();
        println!(
            "compute profile: {:?} ({})",
            profile.backend, profile.detail
        );
        assert!(!profile.detail.is_empty());
    }
}
