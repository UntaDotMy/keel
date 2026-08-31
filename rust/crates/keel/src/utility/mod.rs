//! Purpose: Thin dispatcher for utility command groups with pub use re-exports.
//! Caller: commands.rs for the anvil, code-search, memory, gain, session, and stats surfaces.
//! Main Functions: Re-exports public functions from submodules.
//! Side Effects: None — pure module organization.

pub mod anvil;
pub mod code_graph;
pub mod code_search;
pub mod config_audit;
pub mod design_intelligence;
pub mod eval;
pub mod gain;
pub mod hashing;
pub mod memory;
pub mod memory_families;
pub mod observe;
pub mod recall;
pub mod record_store;
pub mod session;
pub mod skill_eval;
pub mod skill_lint;
pub mod skill_match;
pub mod skill_usage;
pub(crate) mod sqlite;
pub mod stats;
pub mod system_map;
pub mod working_brief;
pub mod workspace_index;

pub use anvil::run_anvil_command;
pub use code_graph::run_code_graph_command;
pub use code_search::run_code_index_command;
pub use code_search::run_code_search_command;
pub use config_audit::run_config_audit_command;
pub use design_intelligence::run_design_intelligence_command;
pub use eval::run_eval_command;
pub use gain::run_gain_command;
pub use memory::run_memory_command;
pub use observe::run_observe_command;
pub use session::run_session_command;
pub use skill_eval::run_skill_eval_command;
pub use skill_lint::run_skill_lint_command;
pub use stats::run_stats_command;
