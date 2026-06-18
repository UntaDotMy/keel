use crate::proxy::adapter::CommandAdapter;
use crate::proxy::command_ast::CommandAst;

pub struct AdapterRegistry {
    adapters: Vec<Box<dyn CommandAdapter>>,
}

impl AdapterRegistry {
    pub fn new() -> Self {
        Self {
            adapters: Vec::new(),
        }
    }

    pub fn register(&mut self, adapter: Box<dyn CommandAdapter>) {
        self.adapters.push(adapter);
    }

    pub fn find_adapter(&self, ast: &CommandAst) -> Option<&dyn CommandAdapter> {
        for adapter in &self.adapters {
            if adapter.matches(ast) {
                return Some(adapter.as_ref());
            }
        }
        None
    }

    pub fn best_match(&self, ast: &CommandAst) -> Option<&dyn CommandAdapter> {
        self.find_adapter(ast)
    }

    pub fn find_by_name(&self, name: &str) -> Option<&dyn CommandAdapter> {
        self.adapters
            .iter()
            .find(|adapter| adapter.name() == name)
            .map(|adapter| adapter.as_ref())
    }
}

impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::new()
    }
}
