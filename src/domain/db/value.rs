#[derive(Debug, Clone, PartialEq)]
pub enum DbValue {
    Null,
    Text(String),
    Integer(i64),
    Float(f64),
    Bool(bool),
    Json(String),
}
