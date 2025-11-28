#[derive(thiserror::Error, Debug, Clone)]
pub enum AuditError {
    #[error("Audit Validation: {0}")]
    Validation(String),
    #[error("Audit Storage: {0}")]
    Storage(String),
}
