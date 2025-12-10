use super::*;

fn config() -> ServiceTokenSection {
    ServiceTokenSection {
        lifetime_seconds: 2,
        idle_timeout_seconds: 1,
        cleanup_interval_seconds: 1,
    }
}

#[test]
fn issue_validate_and_revoke() {
    let store = ServiceTokenStore::new(&config());
    let request = DelegatedTokenRequest::new(
        DelegatedActor::Service {
            service_id: "module:test".to_string(),
            role: ServiceRole::Write,
        },
        "tenant-1",
    );
    let issued = store.issue(request).expect("token issued");
    let validated = store.validate(&issued.token).expect("token validates");
    assert_eq!(validated.tenant_id, "tenant-1");
    store.revoke(&issued.token).expect("revoke ok");
    assert!(matches!(
        store.validate(&issued.token),
        Err(ServiceTokenError::NotFound)
    ));
}

#[test]
fn idle_timeout_expires_tokens() {
    let store = ServiceTokenStore::new(&config());
    let issued = store
        .issue(DelegatedTokenRequest::new(
            DelegatedActor::Service {
                service_id: "module:test".to_string(),
                role: ServiceRole::Read,
            },
            "tenant-1",
        ))
        .expect("token issued");
    std::thread::sleep(Duration::from_millis(1100));
    assert!(matches!(
        store.validate(&issued.token),
        Err(ServiceTokenError::IdleTimeout | ServiceTokenError::NotFound)
    ));
}
