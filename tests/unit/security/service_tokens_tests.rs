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

// --- Concurrency (step 5.6): read lock on the hot path, exact revocation ---

fn long_lived() -> ServiceTokenSection {
    ServiceTokenSection {
        lifetime_seconds: 600,
        idle_timeout_seconds: 600,
        cleanup_interval_seconds: 600,
        refresh_grace_seconds: 600,
    }
}

/// `validate` must not need the write lock: with a read guard held by
/// someone else it still answers. Before 5.6 it took the write lock on
/// every call and blocked here until the guard was released.
#[test]
fn validate_runs_under_a_concurrent_read_guard() {
    let store = std::sync::Arc::new(ServiceTokenStore::new(&long_lived()));
    let issued = store
        .issue(service_request("module:read-lock"))
        .expect("token issued");
    let guard = store.tokens.read().expect("read guard");
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = {
        let store = std::sync::Arc::clone(&store);
        let token = issued.token.clone();
        std::thread::spawn(move || {
            let result = store.validate(&token).map(|claims| claims.tenant_id);
            let _ = tx.send(result.is_ok());
        })
    };
    let answered = rx.recv_timeout(Duration::from_secs(2));
    drop(guard);
    worker.join().expect("worker joined");
    assert_eq!(
        answered,
        Ok(true),
        "validate blocked behind a read guard: it takes the write lock"
    );
}

/// Validation keeps `last_activity` current although it no longer holds the
/// write lock: a token used more often than its idle timeout never idles out.
#[test]
fn validation_under_read_lock_still_refreshes_idle_time() {
    let cfg = ServiceTokenSection {
        lifetime_seconds: 60,
        idle_timeout_seconds: 1,
        cleanup_interval_seconds: 60,
        refresh_grace_seconds: 0,
    };
    let store = ServiceTokenStore::new(&cfg);
    let issued = store
        .issue(service_request("module:idle"))
        .expect("token issued");
    for _ in 0..6 {
        std::thread::sleep(Duration::from_millis(400));
        store.validate(&issued.token).expect("still active");
    }
    std::thread::sleep(Duration::from_millis(1300));
    assert!(matches!(
        store.validate(&issued.token),
        Err(ServiceTokenError::IdleTimeout)
    ));
    assert!(matches!(
        store.validate(&issued.token),
        Err(ServiceTokenError::NotFound)
    ));
}

/// No validation that starts after `revoke` returned may succeed, however
/// many threads validate the same token at the same time.
#[test]
fn revocation_is_exact_under_parallel_validation() {
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::Arc;

    let store = Arc::new(ServiceTokenStore::new(&long_lived()));
    let issued = store
        .issue(service_request("module:revoke"))
        .expect("token issued");
    let revoked = Arc::new(AtomicBool::new(false));
    let successes = Arc::new(AtomicUsize::new(0));
    let late_successes = Arc::new(AtomicUsize::new(0));
    let workers: Vec<_> = (0..16)
        .map(|_| {
            let store = Arc::clone(&store);
            let token = issued.token.clone();
            let revoked = Arc::clone(&revoked);
            let successes = Arc::clone(&successes);
            let late = Arc::clone(&late_successes);
            std::thread::spawn(move || loop {
                let after_revoke = revoked.load(std::sync::atomic::Ordering::SeqCst);
                match store.validate(&token) {
                    Ok(_) if after_revoke => {
                        late.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                    Ok(_) => {
                        successes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                    // Revoked meanwhile (or before): this thread is done.
                    Err(ServiceTokenError::NotFound) => break,
                    Err(other) => panic!("unexpected error {other:?}"),
                }
            })
        })
        .collect();
    while successes.load(std::sync::atomic::Ordering::SeqCst) < 2_000 {
        std::thread::yield_now();
    }
    store.revoke(&issued.token).expect("revoked");
    revoked.store(true, std::sync::atomic::Ordering::SeqCst);
    for worker in workers {
        worker.join().expect("worker joined");
    }
    assert_eq!(late_successes.load(std::sync::atomic::Ordering::SeqCst), 0);
}

/// Expiry stays exact: no successful validation returns claims that were
/// already expired when the call began.
#[test]
fn expiry_is_exact_under_parallel_validation() {
    use std::sync::Arc;
    let cfg = ServiceTokenSection {
        lifetime_seconds: 1,
        idle_timeout_seconds: 60,
        cleanup_interval_seconds: 0,
        refresh_grace_seconds: 0,
    };
    let store = Arc::new(ServiceTokenStore::new(&cfg));
    let issued = store
        .issue(service_request("module:expiry"))
        .expect("token issued");
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let store = Arc::clone(&store);
            let token = issued.token.clone();
            std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + Duration::from_millis(1500);
                let mut stale = 0usize;
                while std::time::Instant::now() < deadline {
                    let before = OffsetDateTime::now_utc();
                    if let Ok(claims) = store.validate(&token) {
                        if claims.expires_at <= before {
                            stale += 1;
                        }
                    }
                }
                stale
            })
        })
        .collect();
    let stale: usize = workers
        .into_iter()
        .map(|worker| worker.join().expect("worker joined"))
        .sum();
    assert_eq!(stale, 0);
    assert!(store.validate(&issued.token).is_err());
}

/// Measurement, not a gate: validations per second with 1, 5 and 20
/// threads on one token. Run with
/// `cargo test --lib service_tokens -- --ignored --nocapture`.
#[test]
#[ignore = "measurement"]
fn measure_parallel_validation_throughput() {
    use std::sync::Arc;
    let store = Arc::new(ServiceTokenStore::new(&long_lived()));
    let issued = store
        .issue(service_request("module:bench"))
        .expect("token issued");
    for threads in [1usize, 5, 20] {
        let window = Duration::from_millis(1500);
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                let store = Arc::clone(&store);
                let token = issued.token.clone();
                std::thread::spawn(move || {
                    let deadline = std::time::Instant::now() + window;
                    let mut n = 0u64;
                    while std::time::Instant::now() < deadline {
                        store.validate(&token).expect("valid");
                        n += 1;
                    }
                    n
                })
            })
            .collect();
        let total: u64 = workers
            .into_iter()
            .map(|worker| worker.join().expect("worker joined"))
            .sum();
        let per_sec = total as f64 / window.as_secs_f64();
        println!(
            "validate threads={threads:>2} total={per_sec:>12.0}/s per_thread={:>12.0}/s",
            per_sec / threads as f64
        );
    }
}
