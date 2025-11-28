use super::id::ModuleId;
use super::version::ModuleVersion;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSummary {
    pub id: ModuleId,
    pub version: ModuleVersion,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
}
