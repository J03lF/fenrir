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
fn enforce_guard_allows_force_with_trailing_semicolon() {
    let stmt = "DELETE FROM foo --force;";
    let sanitized = enforce_guard(stmt).expect("force should allow with semicolon");
    assert!(sanitized.contains("DELETE FROM foo"));
    assert!(!sanitized.contains("--force"));
}

#[test]
fn enforce_guard_blocks_without_force() {
    assert!(enforce_guard("DROP TABLE foo").is_err());
}

#[test]
fn normalize_meta_command_trims_semicolons() {
    assert_eq!(normalize_meta_command("exit;"), "exit");
    assert_eq!(normalize_meta_command("  help;;  "), "help");
    assert_eq!(normalize_meta_command("refresh  ;;;"), "refresh");
}

#[test]
fn normalize_meta_command_preserves_inner_content() {
    assert_eq!(
        normalize_meta_command("select * from foo;"),
        "select * from foo"
    );
    assert_eq!(normalize_meta_command(r"\d users;"), r"\d users");
}
