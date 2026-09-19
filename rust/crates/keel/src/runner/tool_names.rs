//! Purpose: Canonical host tool-name vocabulary shared by every adapter path.
//! Caller: hook_lifecycle (gate, post-tool, counters), observation, shell_rewrite, bridge.
//! Dependencies: none beyond std.
//! Main Functions: normalize_tool_name, is_edit_class_tool, is_shell_tool_name.
//! Side Effects: None (pure classification).
//!
//! Hosts disagree about what their tools are called: Claude Code reports `Edit`,
//! Command Code reports `edit_file` and `write_file`, Antigravity
//! reports `write_to_file`, Cursor reports `StrReplace`, Codex reports
//! `apply_patch`. Classifying on the raw string meant every host whose names were
//! missing from the list silently skipped the Iron Law gate and the `keel run`
//! rewrite with no error at all. That is a fail-open hole, not a cosmetic gap,
//! because a skipped classification reads exactly like a clean tool call.
//!
//! One normalizer plus one vocabulary keeps every adapter on the same path.
//! Entries are stored separator-free (`writefile`, not `write_file`) so
//! `multi_edit`, `multi-edit`, and `MultiEdit` resolve together and the lists
//! cannot drift apart on punctuation alone.

/// Lowercase and drop every non-alphanumeric character.
///
/// Host naming differs mostly in case and separators, so removing both collapses
/// `StrReplace`, `str_replace`, and `str-replace` onto one key. A name that
/// normalizes to the empty string is never a match.
pub fn normalize_tool_name(tool_name: &str) -> String {
    tool_name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect()
}

/// File-mutating tools. These are the Iron Law gate's edit class, and they also
/// drive comment lint, graph context, and the edit-class counters.
pub const EDIT_CLASS_TOOL_NAMES: &[&str] = &[
    // Claude Code, OpenCode, Pi, OMP, and the Claude-compatible hosts.
    "edit",
    "write",
    "multiedit",
    "notebookedit",
    // Codex.
    "applypatch",
    // Cursor: Write|Edit|Delete|StrReplace|MultiEdit|NotebookEdit|ApplyPatch|Patch|SearchReplace.
    "delete",
    "strreplace",
    "patch",
    "searchreplace",
    // Grok maps Claude Edit/Write/MultiEdit onto search_replace (covered above).
    // Antigravity.
    "writetofile",
    "replacefilecontent",
    "multireplacefilecontent",
    // Command Code.
    "editfile",
    "writefile",
    // Widely used file-tool aliases seen across harnesses and MCP file servers.
    "createfile",
    "deletefile",
    "strreplaceeditor",
    "multieditfile",
];

/// Tools that carry a shell command string.
///
/// This list is deliberately wider than
/// [`crate::runner::shell_rewrite::SHELL_TOOL_NAMES`]: the gate only needs to
/// know that the call is shell work, while a rewrite additionally needs to know
/// which shell will parse the result. A name that is gated here but has no
/// rewrite mapping keeps the Iron Law protection without risking a
/// wrongly-quoted command.
pub const SHELL_TOOL_NAMES: &[&str] = &[
    // POSIX shells and Claude Code / OpenCode / Pi / OMP `Bash`.
    "bash",
    "shell",
    "sh",
    "zsh",
    "fish",
    // Windows shells.
    "powershell",
    "pwsh",
    "cmd",
    // Command Code.
    "shellcommand",
    // Cursor: Shell|Bash|PowerShell|Command|Terminal.
    "command",
    "terminal",
    // Claude-compatible and host-neutral shell names.
    "runcommand",
    "runterminalcommand",
    "execcommand",
    "localshell",
    "unifiedexec",
];

/// Whether `tool_name` mutates files on any supported host.
pub fn is_edit_class_tool(tool_name: &str) -> bool {
    let normalized = normalize_tool_name(tool_name);
    !normalized.is_empty() && EDIT_CLASS_TOOL_NAMES.contains(&normalized.as_str())
}

/// Whether `tool_name` carries a shell command on any supported host.
pub fn is_shell_tool_name(tool_name: &str) -> bool {
    let normalized = normalize_tool_name(tool_name);
    !normalized.is_empty() && SHELL_TOOL_NAMES.contains(&normalized.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_collapses_case_and_separators() {
        assert_eq!(normalize_tool_name("MultiEdit"), "multiedit");
        assert_eq!(normalize_tool_name("multi_edit"), "multiedit");
        assert_eq!(normalize_tool_name("multi-edit"), "multiedit");
        assert_eq!(normalize_tool_name("  write_file  "), "writefile");
        assert_eq!(normalize_tool_name("!!!"), "");
    }

    /// Every host's real edit-class names must classify. These are the names the
    /// adapters forward; a miss here is a silent gate bypass on that host.
    #[test]
    fn every_host_edit_tool_is_recognized() {
        for tool in [
            // Claude Code.
            "Edit",
            "Write",
            "MultiEdit",
            "NotebookEdit",
            // Codex.
            "apply_patch",
            // Cursor.
            "StrReplace",
            "SearchReplace",
            "Delete",
            "Patch",
            // Grok.
            "search_replace",
            // Antigravity.
            "write_to_file",
            "replace_file_content",
            "multi_replace_file_content",
            // Command Code.
            "edit_file",
            "write_file",
            // OpenCode / Pi / OMP.
            "edit",
            "write",
        ] {
            assert!(
                is_edit_class_tool(tool),
                "{tool} must classify as edit-class or that host skips the Iron Law gate"
            );
        }
    }

    /// Every host's real shell-carrying names must classify, or that host never
    /// reaches the Iron Law shell gate.
    #[test]
    fn every_host_shell_tool_is_recognized() {
        for tool in [
            // Claude Code / OpenCode / Pi / OMP.
            "Bash",
            // Windows shells.
            "PowerShell",
            "pwsh",
            "cmd",
            // Command Code.
            "shell_command",
            // Cursor.
            "Shell",
            "Command",
            "Terminal",
            // Claude-compatible and host-neutral.
            "run_command",
            "run_terminal_command",
        ] {
            assert!(
                is_shell_tool_name(tool),
                "{tool} must classify as shell-class or that host skips the shell gate"
            );
        }
    }

    #[test]
    fn navigation_tools_are_never_gated_as_edits_or_shell() {
        for tool in [
            "Read",
            "read_file",
            "Grep",
            "Glob",
            "read_directory",
            "todo_write",
            "web_fetch",
            "WebSearch",
            // A keel-mounted research device is research, not a file edit.
            "xd://mcp__keel__system_map",
            "",
            "   ",
        ] {
            assert!(!is_edit_class_tool(tool), "{tool} must not be edit-class");
            assert!(!is_shell_tool_name(tool), "{tool} must not be shell-class");
        }
    }

    /// Both lists must stay normalized, lowercase, unique, and non-empty so a
    /// future entry cannot be added in a form the normalizer never produces.
    #[test]
    fn vocabulary_entries_are_normalized_unique_and_nonempty() {
        for (label, list) in [("edit", EDIT_CLASS_TOOL_NAMES), ("shell", SHELL_TOOL_NAMES)] {
            let mut seen = std::collections::BTreeSet::new();
            for entry in list {
                assert!(!entry.is_empty(), "{label} list has an empty entry");
                assert_eq!(
                    *entry,
                    normalize_tool_name(entry),
                    "{label} entry {entry} is not in normalized form"
                );
                assert!(seen.insert(*entry), "{label} entry {entry} is duplicated");
            }
        }
    }
}
