//! Purpose: Thin dispatcher for utility command groups with pub use re-exports
//! Caller: commands.rs for code-search, design-intelligence, memory, memoriesv2, orchestration, workflow, gain, session, and bench
//! Dependencies: submodules code_search, memory, gain, session, system_map
//! Main Functions: Re-exports public functions from submodules
//! Side Effects: None, pure module organization

pub mod checkpoint;
pub mod code_graph;
pub mod code_search;
pub mod config_audit;
pub mod design_intelligence;
pub mod gain;
pub mod memory;
pub mod memory_families;
pub mod recall;
pub mod record_store;
pub mod session;
pub mod skill_lint;
pub mod skill_match;
pub mod sprint;
pub mod system_map;
pub mod user_story;
pub mod workflow_ledger;
pub mod working_brief;

pub use checkpoint::run_checkpoint_command;
pub use code_graph::run_code_graph_command;
pub use code_search::run_code_search_command;
pub use config_audit::run_config_audit_command;
pub use design_intelligence::run_design_intelligence_command;
pub use gain::run_gain_command;
pub use memory::{
    run_bench_command, run_memory_command, run_orchestration_command, run_workflow_command,
};
pub use session::run_session_command;
pub use skill_lint::run_skill_lint_command;
pub use sprint::run_sprint_command;
pub use user_story::run_user_story_command;
