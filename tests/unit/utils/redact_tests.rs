use super::{redact_uri_password, redact_uris_in_text, request_target};

#[test]
fn redacts_password_of_socket_uri() {
    let uri = "postgres://fenrir_embedded:s3cr%2Bt%3D@/postgres?host=/opt/run/db/socket";
    assert_eq!(
        redact_uri_password(uri),
        "postgres://fenrir_embedded:***@/postgres?host=/opt/run/db/socket"
    );
}

#[test]
fn redacts_password_of_tcp_uri() {
    assert_eq!(
        redact_uri_password("postgres://fenrir_embedded:pw@127.0.0.1:55432/postgres"),
        "postgres://fenrir_embedded:***@127.0.0.1:55432/postgres"
    );
}

#[test]
fn redacts_password_with_raw_special_characters() {
    let redacted = redact_uri_password("postgres://u:p@ss/w?rd#x@db.local:5432/app");
    assert_eq!(redacted, "postgres://u:***@db.local:5432/app");
    assert!(!redacted.contains("ss/w?rd"));
}

#[test]
fn redacts_password_query_parameter() {
    assert_eq!(
        redact_uri_password("postgres://u@/db?host=/tmp&Password=hunter2&sslmode=disable"),
        "postgres://u@/db?host=/tmp&Password=***&sslmode=disable"
    );
}

#[test]
fn leaves_uris_without_password_unchanged() {
    for uri in [
        "postgres://fenrir_embedded@/postgres?host=/opt/run/db/socket",
        "postgres://127.0.0.1:55432/postgres",
        "sqlite:///opt/run/db/fenrir.db",
        "not a uri",
    ] {
        assert_eq!(redact_uri_password(uri), uri);
    }
}

#[test]
fn redacts_uris_inside_log_lines() {
    let line = "postgres start failed: postgres://u:topsecret@/postgres?host=/s refused";
    let redacted = redact_uris_in_text(line);
    assert_eq!(
        redacted,
        "postgres start failed: postgres://u:***@/postgres?host=/s refused"
    );
    assert!(!redacted.contains("topsecret"));
    assert_eq!(
        redact_uris_in_text("plain line\twith  tabs"),
        "plain line\twith  tabs"
    );
}

// --- Invite tokens in request targets (5.6, after 3.8a) ---

#[test]
fn masks_invite_tokens_in_paths_and_queries() {
    for (raw, logged) in [
        ("/invite/AbCdEf123", "/invite/<redacted>"),
        (
            "/gateway/services/module%3Aathenos-api%3A%3Aapi-gateway/api/v1/invite/AbCdEf123/join",
            "/gateway/services/module%3Aathenos-api%3A%3Aapi-gateway/api/v1/invite/<redacted>/join",
        ),
        (
            "/api/v1/workspaces/w1/invites/AbCdEf123/revoke",
            "/api/v1/workspaces/w1/invites/<redacted>/revoke",
        ),
        ("/api/v1/INVITE/AbCdEf123", "/api/v1/INVITE/<redacted>"),
        ("/api/v1/invite%2FAbCdEf123", "/api/v1/<redacted>"),
        (
            "/api/v1/invite?token=AbCdEf123&x=1",
            "/api/v1/invite?token=<redacted>&x=1",
        ),
        (
            "/auth/accept?invite=AbCdEf123",
            "/auth/accept?invite=<redacted>",
        ),
        ("/x?Invite_Token=AbCdEf123", "/x?Invite_Token=<redacted>"),
    ] {
        let masked = request_target(raw);
        assert_eq!(masked, logged, "{raw}");
        assert!(!masked.contains("AbCdEf123"), "{raw}");
    }
}

#[test]
fn leaves_other_targets_alone() {
    for raw in [
        "/",
        "/app/inbox",
        "/api/v1/work/tickets?workspace_id=w1&limit=50",
        "/api/v1/workspaces/w1/invites",
        "/invite/",
        "/api/v1/invitee/x",
        "/x?tokens=1&token=",
    ] {
        assert_eq!(request_target(raw), raw);
    }
}
