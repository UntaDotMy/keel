//! Purpose: Thin dispatcher for utility command groups with pub use re-exports
//! Caller: commands.rs for code-search, design-intelligence, memory, memoriesv2, orchestration, workflow, gain, session, and bench
//! Dependencies: submodules code_search, memory, gain, session, system_map
//! Main Functions: Re-exports public functions from submodules
//! Side Effects: None, pure module organization

pub mod code_search;
pub mod gain;
pub mod memory;
pub mod recall;
pub mod session;
pub mod system_map;
pub mod workflow_ledger;
pub mod working_brief;

pub use code_search::{run_code_search_command, run_design_intelligence_command};
pub use gain::run_gain_command;
pub use memory::{
    run_bench_command, run_memory_command, run_orchestration_command, run_workflow_command,
};
pub use session::run_session_command;
