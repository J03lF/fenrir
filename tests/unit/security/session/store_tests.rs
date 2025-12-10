use super::*;
use crate::security::auth::Role;
use std::thread;

fn config() -> SessionSection {
    SessionSection {
        lifetime_seconds: 2,
        idle_timeout_seconds: 1,
        cleanup_interval_seconds: 1,
    }
}

#[test]
fn session_issue_validate_and_revoke() {
    let store = SessionStore::new(&config());
    let session = store
        .create_session("test-user", Role::Viewer)
        .expect("session should be created");
    let validated = store.validate(&session.id).expect("session must validate");
    assert_eq!(validated.user_id, "test-user");
    assert_eq!(validated.role, Role::Viewer);

    store
        .revoke(&session.id)
        .expect("revoking valid session should succeed");
    assert!(matches!(
        store.validate(&session.id),
        Err(SessionError::NotFound | SessionError::Expired | SessionError::IdleTimeout)
    ));
}

#[test]
fn session_idle_timeout_expires() {
    let store = SessionStore::new(&config());
    let session = store
        .create_session("idle-user", Role::Operator)
        .expect("session should be created");
    thread::sleep(Duration::from_secs(1));
    assert!(matches!(
        store.validate(&session.id),
        Err(SessionError::IdleTimeout | SessionError::NotFound)
    ));
}
