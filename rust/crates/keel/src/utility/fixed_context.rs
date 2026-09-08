//! Runtime `o200k_base` inventory and budget gate for fixed context surfaces.
//! Every actual value is recomputed; only reviewed budgets are constants.

use std::fs;
use std::path::Path;

use crate::proxy::token_meter::TokenMeter;

pub(crate) const TOKENIZER: &str = "o200k_base";
pub(crate) const REPRODUCTION_COMMAND: &str = "keel stats --json --workspace-root <repo>";
pub(crate) const SURFACE_COUNT: usize = 19;

pub(crate) const SIMPLE_PROMPT_FIXTURE: &str = "hello";
pub(crate) const CODE_CHANGE_PROMPT_FIXTURE: &str = "fix the bug";

pub(crate) const PLANNER_POINTER: &str = "Planner: read plan.packet.json before implementation.";
pub(crate) const RESEARCH_POINTER: &str =
    "Research: read research.packet.json and verify freshness before claims.";
pub(crate) const TICKET_CHECKLIST_POINTER: &str =
    "Tickets: read tickets.json and complete every unchecked item.";
pub(crate) const WARNING_STATUS_POINTER: &str = "Warnings: read warnings.json before closeout.";
pub(crate) const UI_VERIFICATION_POINTER: &str =
    "UI: read ui-verification.json and attach runtime evidence before closeout.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixedContextEntry {
    pub surface: &'static str,
    pub actual_tokens: usize,
    pub budget_tokens: usize,
    pub source_available: bool,
}

impl FixedContextEntry {
    pub fn status(&self) -> &'static str {
        if !self.source_available {
            "source_missing"
        } else if self.actual_tokens > self.budget_tokens {
            "exceeded"
        } else {
            "within_budget"
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FixedContextMcpSummary {
    pub tool_count: usize,
    pub eager_tool_count: usize,
    pub deferred_tool_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FixedContextSkillSummary {
    pub skill_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixedContextLedger {
    pub entries: Vec<FixedContextEntry>,
    pub mcp: FixedContextMcpSummary,
    pub skills: FixedContextSkillSummary,
}

impl FixedContextLedger {
    pub fn status(&self) -> &'static str {
        if self
            .entries
            .iter()
            .any(|entry| entry.status() == "source_missing")
        {
            "source_missing"
        } else if self
            .entries
            .iter()
            .any(|entry| entry.status() == "exceeded")
        {
            "exceeded"
        } else {
            "within_budget"
        }
    }
}

fn ratified_budget(surface: &str) -> usize {
    // Ratified from the first hermetic Phase 1 runtime measurement as
    // ceil(actual_tokens * 1.10). Actual values remain runtime-computed.
    match surface {
        "mcp.tools_list.catalog" => 3_174,
        "hook.session_start.bootstrap" => 503,
        "hook.user_prompt_submit.simple" => 113,
        "hook.user_prompt_submit.code_change" => 169,
        "repo.CLAUDE.md" => 10_897,
        "repo.AGENTS.md" => 2_583,
        "repo.WORKFLOW.md" => 5_373,
        "generated.claude.CLAUDE.md" => 1_061,
        "generated.codex.AGENTS.md" => 401,
        "generated.omp.AGENTS.md" => 145,
        "generated.zcode.AGENTS.md" => 143,
        "generated.antigravity.GEMINI.md" => 145,
        "skills.inline_catalog" => 11_161,
        "skills.full_bodies" => 96_725,
        "pointer.planner" => 10,
        "pointer.research" => 14,
        "pointer.ticket_checklist" => 13,
        "pointer.warning_status" => 10,
        "pointer.ui_verification" => 17,
        _ => 0,
    }
}

fn measured(surface: &'static str, text: &str) -> FixedContextEntry {
    FixedContextEntry {
        surface,
        actual_tokens: TokenMeter::count_text(text),
        budget_tokens: ratified_budget(surface),
        source_available: true,
    }
}

fn measured_file(
    surface: &'static str,
    workspace_root: &Path,
    relative_path: &str,
) -> FixedContextEntry {
    match fs::read_to_string(workspace_root.join(relative_path)) {
        Ok(text) => measured(surface, &text),
        Err(_) => FixedContextEntry {
            surface,
            actual_tokens: 0,
            budget_tokens: ratified_budget(surface),
            source_available: false,
        },
    }
}

pub(crate) fn collect(workspace_root: &Path) -> FixedContextLedger {
    let mcp = crate::mcp::tools_list_context_snapshot();
    let skills = crate::utility::skill_match::skill_context_snapshot(workspace_root);
    let (skill_count, inline_catalog, full_bodies, skills_available) = match skills {
        Some(snapshot) => (
            snapshot.skill_count,
            snapshot.inline_catalog,
            snapshot.full_bodies,
            true,
        ),
        None => (0, String::new(), String::new(), false),
    };

    let mut entries = vec![
        FixedContextEntry {
            surface: "mcp.tools_list.catalog",
            actual_tokens: mcp.catalog_tokens,
            budget_tokens: ratified_budget("mcp.tools_list.catalog"),
            source_available: true,
        },
        measured(
            "hook.session_start.bootstrap",
            crate::runner::hook_lifecycle::session_start_bootstrap(),
        ),
        measured(
            "hook.user_prompt_submit.simple",
            &crate::runner::hook_lifecycle::user_prompt_submit_context(SIMPLE_PROMPT_FIXTURE),
        ),
        measured(
            "hook.user_prompt_submit.code_change",
            &crate::runner::hook_lifecycle::user_prompt_submit_context(CODE_CHANGE_PROMPT_FIXTURE),
        ),
        measured_file("repo.CLAUDE.md", workspace_root, "CLAUDE.md"),
        measured_file("repo.AGENTS.md", workspace_root, "AGENTS.md"),
        measured_file("repo.WORKFLOW.md", workspace_root, "WORKFLOW.md"),
        measured(
            "generated.claude.CLAUDE.md",
            &crate::manager::install::managed_claude_md_block(),
        ),
        measured(
            "generated.codex.AGENTS.md",
            &crate::manager::install::managed_codex_agents_block(),
        ),
        measured(
            "generated.omp.AGENTS.md",
            &crate::manager::install::managed_host_agents_block("Oh My Pi"),
        ),
        measured(
            "generated.zcode.AGENTS.md",
            &crate::manager::install::managed_host_agents_block("ZCode"),
        ),
        measured(
            "generated.antigravity.GEMINI.md",
            &crate::manager::install::managed_host_agents_block("Antigravity"),
        ),
    ];
    entries.push(FixedContextEntry {
        surface: "skills.inline_catalog",
        actual_tokens: TokenMeter::count_text(&inline_catalog),
        budget_tokens: ratified_budget("skills.inline_catalog"),
        source_available: skills_available,
    });
    entries.push(FixedContextEntry {
        surface: "skills.full_bodies",
        actual_tokens: TokenMeter::count_text(&full_bodies),
        budget_tokens: ratified_budget("skills.full_bodies"),
        source_available: skills_available,
    });
    entries.extend([
        measured("pointer.planner", PLANNER_POINTER),
        measured("pointer.research", RESEARCH_POINTER),
        measured("pointer.ticket_checklist", TICKET_CHECKLIST_POINTER),
        measured("pointer.warning_status", WARNING_STATUS_POINTER),
        measured("pointer.ui_verification", UI_VERIFICATION_POINTER),
    ]);

    debug_assert_eq!(entries.len(), SURFACE_COUNT);
    FixedContextLedger {
        entries,
        mcp: FixedContextMcpSummary {
            tool_count: mcp.tool_count,
            eager_tool_count: mcp.eager_tool_count,
            deferred_tool_count: mcp.deferred_tool_count,
        },
        skills: FixedContextSkillSummary { skill_count },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_status_distinguishes_missing_and_exceeded_sources() {
        let within = FixedContextEntry {
            surface: "test",
            actual_tokens: 10,
            budget_tokens: 10,
            source_available: true,
        };
        assert_eq!(within.status(), "within_budget");
        assert_eq!(
            FixedContextEntry {
                actual_tokens: 11,
                ..within.clone()
            }
            .status(),
            "exceeded"
        );
        assert_eq!(
            FixedContextEntry {
                source_available: false,
                ..within
            }
            .status(),
            "source_missing"
        );
    }

    #[test]
    fn warning_status_pointer_is_below_the_hard_cap() {
        assert!(TokenMeter::count_text(WARNING_STATUS_POINTER) < 30);
    }
}
