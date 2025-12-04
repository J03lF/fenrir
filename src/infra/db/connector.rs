use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::anyhow;
use serde_json;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
#[cfg(not(unix))]
use tokio::net::{TcpListener, TcpStream};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info, warn};

use crate::services::db_connector::{DbConnectorEndpoint, DbConnectorService};
use crate::utils::messages::infra::db::connector as connector_messages;
use fenrir_module_kit::{DbConnectorRequest, DbConnectorResponse};

const MAX_REQUEST_SIZE: usize = 64 * 1024;

pub async fn start_connector_server(
    runtime_dir: &Path,
    service: Arc<DbConnectorService>,
) -> anyhow::Result<DbConnectorEndpoint> {
    #[cfg(unix)]
    {
        start_ipc_server(runtime_dir, service).await
    }
    #[cfg(not(unix))]
    {
        start_tcp_server(service).await
    }
}

#[cfg(unix)]
async fn start_ipc_server(
    runtime_dir: &Path,
    service: Arc<DbConnectorService>,
) -> anyhow::Result<DbConnectorEndpoint> {
    let socket_dir = runtime_dir.join("ipc");
    tokio::fs::create_dir_all(&socket_dir).await?;
    let socket_path = socket_dir.join("db-connector.sock");
    if socket_path.exists() {
        tokio::fs::remove_file(&socket_path).await?;
    }
    let listener = UnixListener::bind(&socket_path)?;
    spawn_accept_loop(
        async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _addr)) => {
                        spawn_connection(stream, Arc::clone(&service));
                    }
                    Err(err) => {
                        error!(error = %err, "{}", connector_messages::ACCEPT_FAILED);
                        break;
                    }
                }
            }
        },
        connector_messages::TASK_IPC,
    );
    info!(
        path = %socket_path.display(),
        "{}",
        connector_messages::IPC_READY
    );
    Ok(DbConnectorEndpoint::Ipc {
        path: PathBuf::from(socket_path),
    })
}

#[cfg(unix)]
fn spawn_connection(stream: UnixStream, service: Arc<DbConnectorService>) {
    tokio::spawn(async move {
        if let Err(err) = process_stream(stream, service).await {
            warn!(error = %err, "{}", connector_messages::STREAM_FAILED);
        }
    });
}

#[cfg(not(unix))]
async fn start_tcp_server(service: Arc<DbConnectorService>) -> anyhow::Result<DbConnectorEndpoint> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener
        .local_addr()
        .map_err(|err| anyhow!("{}: {err}", connector_messages::addr_unknown()))?;
    spawn_accept_loop(
        async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        spawn_tcp_connection(stream, Arc::clone(&service));
                    }
                    Err(err) => {
                        error!(error = %err, "{}", connector_messages::ACCEPT_FAILED);
                        break;
                    }
                }
            }
        },
        connector_messages::TASK_TCP,
    );
    info!(%addr, "{}", connector_messages::TCP_READY);
    Ok(DbConnectorEndpoint::Tcp { addr })
}

fn spawn_accept_loop(
    task: impl futures::Future<Output = ()> + Send + 'static,
    label: &'static str,
) {
    tokio::spawn(async move {
        info!(task = label, "{}", connector_messages::TASK_STARTED);
        task.await;
        info!(task = label, "{}", connector_messages::TASK_EXITED);
    });
}

#[cfg(not(unix))]
fn spawn_tcp_connection(stream: TcpStream, service: Arc<DbConnectorService>) {
    tokio::spawn(async move {
        if let Err(err) = process_stream(stream, service).await {
            warn!(error = %err, "{}", connector_messages::STREAM_FAILED);
        }
    });
}

async fn process_stream<S>(mut stream: S, service: Arc<DbConnectorService>) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await?;
    if buf.len() > MAX_REQUEST_SIZE {
        let response = DbConnectorResponse::err(connector_messages::request_too_large());
        write_response(stream, response).await?;
        return Err(anyhow!(connector_messages::request_too_large()));
    }
    let response = match serde_json::from_slice::<DbConnectorRequest>(&buf) {
        Ok(request) => service.execute(request).await,
        Err(err) => DbConnectorResponse::err(connector_messages::request_invalid(err)),
    };
    write_response(stream, response).await?;
    Ok(())
}

async fn write_response<S>(mut stream: S, response: DbConnectorResponse) -> anyhow::Result<()>
where
    S: AsyncWrite + Unpin,
{
    let payload = serde_json::to_vec(&response)?;
    stream.write_all(&payload).await?;
    stream.shutdown().await?;
    Ok(())
}
