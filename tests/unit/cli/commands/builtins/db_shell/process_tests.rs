use super::*;

#[test]
fn guard_detection() {
    assert!(requires_guard("drop table"));
    assert!(requires_guard("DELETE FROM foo"));
    assert!(!requires_guard("select * from foo"));
}

#[test]
fn enforce_guard_allows_force() {
    let stmt = "DROP TABLE foo; --force";
    let sanitized = enforce_guard(stmt).expect("force should allow");
    assert!(sanitized.contains("DROP TABLE"));
    assert!(!sanitized.contains("--force"));
}

#[test]
fn enforce_guard_blocks_without_force() {
    assert!(enforce_guard("DROP TABLE foo").is_err());
}
