use super::is_statement_complete;

#[test]
fn statement_complete_on_trailing_semicolon() {
    assert!(is_statement_complete("SELECT 1;"));
}

#[test]
fn statement_complete_with_force_after_semicolon() {
    assert!(is_statement_complete("DROP TABLE foo; --force"));
}

#[test]
fn statement_incomplete_without_semicolon_or_force() {
    assert!(!is_statement_complete("SELECT 1"));
}

#[test]
fn statement_incomplete_with_force_but_no_semicolon() {
    assert!(!is_statement_complete("DROP TABLE foo --force"));
}
