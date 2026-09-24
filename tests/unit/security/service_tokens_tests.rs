use super::*;

fn config() -> ServiceTokenSection {
    ServiceTokenSection {
        lifetime_seconds: 2,
        idle_timeout_seconds: 1,
        cleanup_interval_seconds: 1,
        refresh_grace_seconds: 5,
    }
}

fn service_request(id: &str) -> DelegatedTokenRequest {
    DelegatedTokenRequest::new(
        DelegatedActor::Service {
            service_id: id.to_string(),
            role: ServiceRole::Write,
        },
        "tenant-1",
    )
}

#[test]
fn issue_validate_and_revoke() {
    let store = ServiceTokenStore::new(&config());
    let issued = store
        .issue(service_request("module:test"))
        .expect("token issued");
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
        .issue(service_request("module:test"))
        .expect("token issued");
    std::thread::sleep(Duration::from_millis(1100));
    assert!(matches!(
        store.validate(&issued.token),
        Err(ServiceTokenError::IdleTimeout | ServiceTokenError::NotFound)
    ));
}

// --- Grace-period tests ---

#[test]
fn validate_strict_rejects_expired_token() {
    let cfg = ServiceTokenSection {
        lifetime_seconds: 1,
        idle_timeout_seconds: 10,
        cleanup_interval_seconds: 10,
        refresh_grace_seconds: 10,
    };
    let store = ServiceTokenStore::new(&cfg);
    let issued = store
        .issue(service_request("module:strict"))
        .expect("token issued");
    std::thread::sleep(Duration::from_millis(1200));
    assert!(
        matches!(
            store.validate(&issued.token),
            Err(ServiceTokenError::Expired)
        ),
        "strict validate must reject expired tokens even within grace window"
    );
}

#[test]
fn validate_for_refresh_accepts_expired_within_grace() {
    let cfg = ServiceTokenSection {
        lifetime_seconds: 1,
        idle_timeout_seconds: 10,
        cleanup_interval_seconds: 0,
        refresh_grace_seconds: 10,
    };
    let store = ServiceTokenStore::new(&cfg);
    let issued = store
        .issue(service_request("module:grace"))
        .expect("token issued");
    std::thread::sleep(Duration::from_millis(1200));
    // Token is expired (>1s) but within grace (10s).
    // validate_for_refresh triggers cleanup internally — token must survive.
    let result = store.validate_for_refresh(&issued.token);
    assert!(
        result.is_ok(),
        "validate_for_refresh must accept expired tokens within grace window, got: {result:?}"
    );
    assert_eq!(result.unwrap().tenant_id, "tenant-1");
}

#[test]
fn validate_for_refresh_rejects_beyond_grace() {
    let cfg = ServiceTokenSection {
        lifetime_seconds: 1,
        idle_timeout_seconds: 10,
        cleanup_interval_seconds: 10,
        refresh_grace_seconds: 1,
    };
    let store = ServiceTokenStore::new(&cfg);
    let issued = store
        .issue(service_request("module:beyond"))
        .expect("token issued");
    std::thread::sleep(Duration::from_millis(2200));
    assert!(
        matches!(
            store.validate_for_refresh(&issued.token),
            Err(ServiceTokenError::Expired | ServiceTokenError::NotFound)
        ),
        "validate_for_refresh must reject tokens past grace window"
    );
}

#[test]
fn cleanup_preserves_tokens_within_grace() {
    let cfg = ServiceTokenSection {
        lifetime_seconds: 1,
        idle_timeout_seconds: 10,
        cleanup_interval_seconds: 0,
        refresh_grace_seconds: 10,
    };
    let store = ServiceTokenStore::new(&cfg);
    let token_a = store
        .issue(service_request("module:a"))
        .expect("token a issued");
    let token_b = store
        .issue(service_request("module:b"))
        .expect("token b issued");
    std::thread::sleep(Duration::from_millis(1200));
    // Both tokens expired.  validate_for_refresh on A triggers cleanup —
    // B must also survive because cleanup uses the grace window.
    assert!(
        store.validate_for_refresh(&token_a.token).is_ok(),
        "token A must be refreshable within grace"
    );
    assert!(
        store.validate_for_refresh(&token_b.token).is_ok(),
        "token B must survive cleanup triggered by token A refresh"
    );
    // Strict validate still rejects the expired token
    assert!(
        matches!(
            store.validate(&token_b.token),
            Err(ServiceTokenError::Expired)
        ),
        "strict validate must still reject expired token B"
    );
}
