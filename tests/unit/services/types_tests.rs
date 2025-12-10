use super::*;
use time::OffsetDateTime;

#[test]
fn security_allows_roles_and_scopes() {
    let metadata = ServiceSecurityMetadata {
        internal_only: true,
        allowed_roles: vec![ServiceRole::Write],
        required_scopes: vec![ServiceScope::new("tickets:read").expect("scope")],
        tenant: ServiceTenantGuard::any(),
    };
    let claims = DelegatedTokenClaims {
        token_id: "id".to_string(),
        actor: DelegatedActor::Service {
            service_id: "module:test".to_string(),
            role: ServiceRole::Write,
        },
        tenant_id: "tenant".to_string(),
        scopes: vec![ServiceScope::new("tickets:read").expect("scope")],
        issued_at: OffsetDateTime::now_utc(),
        expires_at: OffsetDateTime::now_utc(),
    };
    assert!(metadata.allows_claims(&claims));
    let mut invalid = claims.clone();
    invalid.actor = DelegatedActor::Service {
        service_id: "module:test".to_string(),
        role: ServiceRole::Read,
    };
    assert!(!metadata.allows_claims(&invalid));
}

#[test]
fn tenant_guard_modes() {
    let guard = ServiceTenantGuard::fixed("tenant-a");
    assert!(guard.allows("tenant-a"));
    assert!(!guard.allows("tenant-b"));
    let list = ServiceTenantGuard::allow_list(vec!["x".into(), "y".into()]);
    assert!(list.allows("x"));
    assert!(!list.allows("z"));
}
