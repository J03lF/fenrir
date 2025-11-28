#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSearchQuery {
    pub pattern: Option<String>,
}

impl ModuleSearchQuery {
    pub fn new(pattern: Option<String>) -> Self {
        Self { pattern }
    }
}
