use super::*;

#[test]
fn service_role_parsing_and_ordering() {
    assert!(ServiceRole::Admin.satisfies(ServiceRole::Write));
    assert!(ServiceRole::Write.satisfies(ServiceRole::Read));
    assert!(!ServiceRole::Read.satisfies(ServiceRole::Write));
    assert_eq!(
        ServiceRole::from_str("service-admin").unwrap(),
        ServiceRole::Admin
    );
    assert!(ServiceRole::from_str("invalid").is_err());
}

#[test]
fn service_scope_validations() {
    let scope = ServiceScope::new("tickets:read").expect("valid scope");
    assert_eq!(scope.as_str(), "tickets:read");
    assert!(ServiceScope::new("").is_err());
    assert!(ServiceScope::new("tickets").is_err());
    assert!(ServiceScope::new("Tickets:Read").is_err());
}
