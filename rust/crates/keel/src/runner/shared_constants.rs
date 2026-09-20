//! Central canonical shared constants and environment variable names across Keel.
//! All modules and adapters must reference these constants directly.

// Gate Control Environment Variables
pub const IRON_LAW_GATE_ENV_VAR: &str = "KEEL_IRON_LAW_GATE";
pub const REVIEW_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_REVIEW_GATE";
pub const REVIEW_GATE_MAX_BLOCKS_ENV_VAR: &str = "CLAUDE_SKILLS_REVIEW_GATE_MAX_BLOCKS";
pub const BRIEF_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_BRIEF_GATE";
pub const BRIEF_GATE_MAX_BLOCKS_ENV_VAR: &str = "CLAUDE_SKILLS_BRIEF_GATE_MAX_BLOCKS";
pub const RESEARCH_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_RESEARCH_GATE";
pub const RESEARCH_GATE_MAX_BLOCKS_ENV_VAR: &str = "CLAUDE_SKILLS_RESEARCH_GATE_MAX_BLOCKS";
pub const COMPLETENESS_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_COMPLETENESS_GATE";
pub const COMPLETENESS_GATE_MAX_BLOCKS_ENV_VAR: &str = "CLAUDE_SKILLS_COMPLETENESS_GATE_MAX_BLOCKS";
pub const MEMORY_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_MEMORY_GATE";
pub const MEMORY_GATE_MAX_BLOCKS_ENV_VAR: &str = "CLAUDE_SKILLS_MEMORY_GATE_MAX_BLOCKS";
pub const LEARNED_SKILL_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_LEARNED_SKILL_GATE";
pub const LEARNED_SKILL_GATE_MAX_BLOCKS_ENV_VAR: &str =
    "CLAUDE_SKILLS_LEARNED_SKILL_GATE_MAX_BLOCKS";
pub const GRAPH_CONTEXT_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_GRAPH_CONTEXT_GATE";
pub const COMMENT_LINT_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_COMMENT_LINT_GATE";

// Tuning, Retention, and Operational Environment Variables
pub const TIMINGS_RETENTION_ENV_VAR: &str = "CLAUDE_SKILLS_TIMINGS_RETENTION_DAYS";
pub const OBSERVATION_RETENTION_ENV_VAR: &str = "CLAUDE_SKILLS_OBSERVATION_RETENTION_DAYS";
pub const RAW_OUTPUT_RETENTION_ENV_VAR: &str = "CLAUDE_SKILLS_RAW_RETENTION_DAYS";
pub const SYSTEM_MAP_REFRESH_INTERVAL_ENV_VAR: &str = "CLAUDE_SKILLS_SYSTEM_MAP_REFRESH_INTERVAL";
pub const MCP_SELF_HEAL_ENV_VAR: &str = "CLAUDE_SKILLS_MCP_SELF_HEAL";
pub const SESSION_CAPTURE_ENV_VAR: &str = "CLAUDE_SKILLS_SESSION_CAPTURE";
pub const COMPRESSION_HINT_ENV_VAR: &str = "CLAUDE_SKILLS_COMPRESSION_HINT";
pub const COMPRESSION_HINT_AFTER_ENV_VAR: &str = "CLAUDE_SKILLS_COMPRESSION_HINT_AFTER";
pub const CLAUDE_TARGET_OVERRIDE_ENV_VAR: &str = "CLAUDE_TARGET_OVERRIDE";
pub const CLAUDE_SKILLS_HOOK_ENV_VAR: &str = "CLAUDE_SKILLS_HOOK";
pub const CLAUDE_SKILLS_AGENT_ENV_VAR: &str = "CLAUDE_SKILLS_AGENT";
pub const KEEL_HOME_ENV_VAR: &str = "KEEL_HOME";

// Hook and Probe Timeout Constants
pub const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 5;
pub const EXTENDED_HOOK_TIMEOUT_SECS: u64 = 10;
pub const EXTENDED_HOOK_TIMEOUT_MS: u64 = 10_000;
pub const GIT_FIELD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
pub const DOCTOR_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

// Plugin manifest configuration options
pub const PLUGIN_REVIEW_STRICTNESS: &str = "CLAUDE_PLUGIN_OPTION_REVIEW_STRICTNESS";
pub const PLUGIN_SYSTEM_MAP_REFRESH_INTERVAL: &str =
    "CLAUDE_PLUGIN_OPTION_SYSTEM_MAP_REFRESH_INTERVAL";
pub const PLUGIN_MEMORY_RETENTION_DAYS: &str = "CLAUDE_PLUGIN_OPTION_MEMORY_RETENTION_DAYS";

// State Marker Directories and Suffixes
pub const IRON_LAW_SATISFIED_DIR: &str = "iron-law-satisfied";
pub const IRON_LAW_LEGACY_GATE_DIR: &str = "iron-law-gate";
pub const REVIEW_GATE_DIR: &str = "review-gate";
pub const BRIEF_GATE_DIR: &str = "brief-gate";
pub const COMPLETENESS_GATE_DIR: &str = "completeness-gate";
pub const SESSION_STARTED_DIR_SUFFIX: &str = "-session-started";

/// Suffix that turns a gate directory into its block-counter directory.
pub const GATE_BLOCKS_SUFFIX: &str = "-blocks";
/// Per-gate block-counter directories. Must stay the gate dir plus the suffix;
/// the unit test at the bottom of this file pins that derivation.
pub const REVIEW_GATE_BLOCKS_DIR: &str = "review-gate-blocks";
pub const COMPLETENESS_GATE_BLOCKS_DIR: &str = "completeness-gate-blocks";

pub const REVIEWED_EXT: &str = ".reviewed";
pub const BRIEFED_EXT: &str = ".briefed";
pub const SCANNED_EXT: &str = ".scanned";

// Gate Decision Literals
pub const GATE_DECISION_BLOCK: &str = "block";
pub const GATE_DECISION_WARN: &str = "warn";
pub const GATE_DECISION_ESCALATE: &str = "escalate";
pub const GATE_DECISION_NUDGE: &str = "nudge";
pub const GATE_DECISION_OFF: &str = "off";
pub const GATE_DECISION_ALLOW: &str = "allow";

// Default Retention and Threshold Values
pub const RAW_OUTPUT_DEFAULT_RETENTION_DAYS: u64 = 14;
pub const TIMINGS_DEFAULT_RETENTION_DAYS: u64 = 30;
pub const OBSERVATION_DEFAULT_RETENTION_DAYS: u64 = 30;
pub const SYSTEM_MAP_REFRESH_DEFAULT_THRESHOLD: u64 = 10;
pub const MANAGED_PRE_TOOL_USE_EVENT: &str = "PreToolUse";

// Claude Code PermissionRequest surface. The tool name and the allow-rule text
// are that host's schema, not a host-neutral shell classification.
pub const CLAUDE_PERMISSION_TOOL_NAME: &str = "Bash";
pub const CLAUDE_PERMISSION_ALLOW_RULE: &str = "Bash(keel *)";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_counter_dirs_derive_from_their_gate_dirs() {
        assert_eq!(
            REVIEW_GATE_BLOCKS_DIR,
            format!("{REVIEW_GATE_DIR}{GATE_BLOCKS_SUFFIX}")
        );
        assert_eq!(
            COMPLETENESS_GATE_BLOCKS_DIR,
            format!("{COMPLETENESS_GATE_DIR}{GATE_BLOCKS_SUFFIX}")
        );
    }
}
