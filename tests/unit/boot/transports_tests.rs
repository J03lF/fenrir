use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::net::{TcpListener, TcpStream};

use super::control_plane_before_modules;

#[tokio::test]
async fn modules_start_only_after_the_control_plane_accepts_connections() {
    let bound: Arc<Mutex<Option<(TcpListener, SocketAddr)>>> = Arc::new(Mutex::new(None));

    let control_plane = {
        let bound = Arc::clone(&bound);
        async move {
            // Give the module future a chance to run first if the order were wrong.
            tokio::task::yield_now().await;
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let addr = listener.local_addr().expect("local addr");
            *bound.lock().expect("lock") = Some((listener, addr));
        }
    };
    let modules = {
        let bound = Arc::clone(&bound);
        async move {
            let addr = bound
                .lock()
                .expect("lock")
                .as_ref()
                .map(|(_, addr)| *addr)
                .expect("control plane must be bound before modules start");
            // Like a module's first token request: the connection must succeed.
            TcpStream::connect(addr)
                .await
                .expect("control plane accepts connections");
        }
    };

    control_plane_before_modules(Some(control_plane), modules).await;
}

#[tokio::test]
async fn modules_still_start_without_an_http_control_plane() {
    let started = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&started);
    control_plane_before_modules(None::<std::future::Ready<()>>, async move {
        flag.store(true, Ordering::SeqCst);
    })
    .await;
    assert!(started.load(Ordering::SeqCst));
}
