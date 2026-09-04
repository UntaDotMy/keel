//! Purpose: Build the command-adapter registry for the token-saving proxy.
//! Caller: proxy::run when preparing a proxied command.
//! Dependencies: Built-in Rust adapters plus optional project filter adapters.
//! Main Functions: build_adapter_registry, build_builtin_adapter_registry.
//! Side Effects: Reads optional project filter configuration files from the current workspace.

use crate::adapters::build::BuildAdapter;
use crate::adapters::cloud::CloudAdapter;
use crate::adapters::containers::ContainersAdapter;
use crate::adapters::database::DatabaseAdapter;
use crate::adapters::files::FilesAdapter;
use crate::adapters::generic::GenericAdapter;
use crate::adapters::git::GitAdapter;
use crate::adapters::lint::LintAdapter;
use crate::adapters::logs::LogsAdapter;
use crate::adapters::search::SearchAdapter;
use crate::adapters::tests::TestAdapter;
use crate::proxy::registry::AdapterRegistry;

pub fn build_adapter_registry() -> AdapterRegistry {
    let mut registry = AdapterRegistry::new();
    for adapter in crate::proxy::filters::load_project_filter_adapters() {
        registry.register(adapter);
    }
    register_builtin_adapters(&mut registry);
    registry
}

pub fn build_builtin_adapter_registry() -> AdapterRegistry {
    let mut registry = AdapterRegistry::new();
    register_builtin_adapters(&mut registry);
    registry
}

fn register_builtin_adapters(registry: &mut AdapterRegistry) {
    registry.register(Box::new(TestAdapter));
    registry.register(Box::new(GitAdapter));
    registry.register(Box::new(SearchAdapter));
    registry.register(Box::new(FilesAdapter));
    registry.register(Box::new(BuildAdapter));
    registry.register(Box::new(LintAdapter));
    registry.register(Box::new(ContainersAdapter));
    registry.register(Box::new(CloudAdapter));
    registry.register(Box::new(DatabaseAdapter));
    registry.register(Box::new(LogsAdapter));
    registry.register(Box::new(GenericAdapter));
}

pub fn adapter_names() -> &'static str {
    "tests, git, search, files, build, lint, containers, cloud, database, logs, project-filter, generic"
}

#[cfg(test)]
mod tests {
    use super::build_adapter_registry;
    use crate::proxy::classify::classify_command;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn registry_selects_specific_adapters_before_generic() {
        let registry = build_adapter_registry();
        let ast = classify_command(&args(&["cargo", "test", "--workspace"])).expect("ast");
        assert_eq!(registry.best_match(&ast).expect("adapter").name(), "tests");

        let ast = classify_command(&args(&["git", "diff", "--cached"])).expect("ast");
        assert_eq!(registry.best_match(&ast).expect("adapter").name(), "git");

        let ast = classify_command(&args(&["rg", "foo", "."])).expect("ast");
        assert_eq!(registry.best_match(&ast).expect("adapter").name(), "search");

        let ast = classify_command(&args(&["cargo", "build"])).expect("ast");
        assert_eq!(registry.best_match(&ast).expect("adapter").name(), "build");

        let ast = classify_command(&args(&["eslint", "."])).expect("ast");
        assert_eq!(registry.best_match(&ast).expect("adapter").name(), "lint");

        let ast = classify_command(&args(&["totally-unknown", "--loud"])).expect("ast");
        assert_eq!(
            registry.best_match(&ast).expect("adapter").name(),
            "generic"
        );
    }

    #[test]
    fn cloud_clis_route_to_cloud_adapter_terraform_stays_logs() {
        let registry = build_adapter_registry();
        for program in ["aws", "az", "gcloud"] {
            let ast = classify_command(&args(&[program, "describe", "x"])).expect("ast");
            assert_eq!(
                registry.best_match(&ast).expect("adapter").name(),
                "cloud",
                "{program} must route to the cloud adapter"
            );
        }
        // terraform deliberately stays on the logs adapter (plan/apply output is
        // not service-API JSON), so it must not be captured by cloud.
        let ast = classify_command(&args(&["terraform", "plan"])).expect("ast");
        assert_eq!(registry.best_match(&ast).expect("adapter").name(), "logs");
    }

    #[test]
    fn database_clients_route_to_database_adapter_dumps_stay_logs() {
        let registry = build_adapter_registry();
        for program in ["psql", "mysql", "sqlite3", "redis-cli", "mongosh"] {
            let ast = classify_command(&args(&[program, "-c", "select 1"])).expect("ast");
            assert_eq!(
                registry.best_match(&ast).expect("adapter").name(),
                "database",
                "{program} must route to the database adapter"
            );
        }
        // Bulk-export tools stream file content, not query results — they stay
        // on the logs adapter rather than the result-table reducer.
        let ast = classify_command(&args(&["pg_dump", "mydb"])).expect("ast");
        assert_eq!(registry.best_match(&ast).expect("adapter").name(), "logs");
    }

    #[test]
    fn forced_adapter_lookup_supports_distinct_build_and_lint_adapters() {
        let registry = build_adapter_registry();
        assert!(registry.find_by_name("build").is_some());
        assert!(registry.find_by_name("lint").is_some());
        assert!(registry.find_by_name("generic").is_some());
        assert!(registry.find_by_name("missing").is_none());
    }
}
