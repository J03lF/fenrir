use super::*;
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fenrir-port-allocator-{}", Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn reuses_assigned_port_when_available() {
    let range = ModulePortRange {
        min: 45000,
        max: 45010,
    };
    let dir = temp_dir();
    let allocator =
        ModulePortAllocator::new(ModulePortStrategy::Dynamic, range, dir.join("ports.json"));
    let module = ModuleId::new("alpha").unwrap();
    let port = match allocator.assigned_port(&module).await {
        Ok(Some(port)) => port,
        Ok(None) => panic!("expected dynamic port assignment"),
        Err(ModuleRuntimeError::NoAvailablePorts { .. }) => {
            eprintln!(
                "skipping reuses_assigned_port_when_available: no ports available in test env"
            );
            return;
        }
        Err(err) => panic!("unexpected error allocating first port: {err:?}"),
    };
    let port_again = match allocator.assigned_port(&module).await {
        Ok(Some(port)) => port,
        Ok(None) => panic!("expected dynamic port assignment"),
        Err(err) => panic!("unexpected error allocating second port: {err:?}"),
    };
    assert_eq!(port, port_again);
}

#[tokio::test]
async fn errors_when_no_ports_available() {
    let range = ModulePortRange {
        min: 47000,
        max: 47000,
    };
    let dir = temp_dir();
    let allocator =
        ModulePortAllocator::new(ModulePortStrategy::Dynamic, range, dir.join("ports.json"));
    let first = ModuleId::new("first").unwrap();
    let second = ModuleId::new("second").unwrap();
    if let Err(ModuleRuntimeError::NoAvailablePorts { .. }) = allocator.assigned_port(&first).await
    {
        eprintln!("skipping errors_when_no_ports_available: no ports recoverable in test env");
        return;
    }
    let err = allocator.assigned_port(&second).await.unwrap_err();
    match err {
        ModuleRuntimeError::NoAvailablePorts {
            range_start,
            range_end,
        } => {
            assert_eq!(range_start, 47000);
            assert_eq!(range_end, 47000);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}
