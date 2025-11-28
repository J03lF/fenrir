pub fn unauthorized() -> &'static str {
    "unauthorized"
}

pub fn forbidden() -> &'static str {
    "forbidden"
}

pub fn unknown_role(value: &str) -> String {
    format!("unknown role '{value}'")
}
