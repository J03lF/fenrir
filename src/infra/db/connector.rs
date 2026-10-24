use std::path::Path;
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
use fenrir_module_kit::{
    DbConnectorRequest, DbConnectorResponse, CONNECTOR_FRAMED_IDLE_TIMEOUT,
    CONNECTOR_FRAMED_PREAMBLE, CONNECTOR_MAX_REQUEST_FRAME,
};

const MAX_REQUEST_SIZE: usize = CONNECTOR_MAX_REQUEST_FRAME;
/// A frame's body must follow its length within this time.
const FRAME_BODY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

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
    Ok(DbConnectorEndpoint::Ipc { path: socket_path })
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

/// One connection from a module.
///
/// - Starting with [`CONNECTOR_FRAMED_PREAMBLE`] (kits since 5.6, when the
///   host announced it via `FENRIR_DB_CONNECTOR_FRAMED`): length-prefixed
///   frames in both directions, one request at a time, until the module
///   closes or stays silent for [`CONNECTOR_FRAMED_IDLE_TIMEOUT`].
/// - Anything else is the legacy one-shot protocol of earlier kits: one JSON
///   request until EOF, one JSON response, close. Unchanged.
async fn process_stream<S>(mut stream: S, service: Arc<DbConnectorService>) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut head = Vec::with_capacity(CONNECTOR_FRAMED_PREAMBLE.len());
    while head.len() < CONNECTOR_FRAMED_PREAMBLE.len()
        && CONNECTOR_FRAMED_PREAMBLE.starts_with(&head)
    {
        let mut chunk = [0u8; 64];
        let wanted = CONNECTOR_FRAMED_PREAMBLE.len() - head.len();
        let read = stream.read(&mut chunk[..wanted]).await?;
        if read == 0 {
            break;
        }
        head.extend_from_slice(&chunk[..read]);
    }
    if head.as_slice() == CONNECTOR_FRAMED_PREAMBLE {
        return process_framed(stream, service).await;
    }
    process_legacy(stream, head, service).await
}

async fn process_legacy<S>(
    mut stream: S,
    head: Vec<u8>,
    service: Arc<DbConnectorService>,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut buf = head;
    (&mut stream)
        .take(u64::try_from(MAX_REQUEST_SIZE).unwrap_or(u64::MAX) + 1)
        .read_to_end(&mut buf)
        .await?;
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

async fn process_framed<S>(mut stream: S, service: Arc<DbConnectorService>) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        let mut header = [0u8; 4];
        match tokio::time::timeout(
            CONNECTOR_FRAMED_IDLE_TIMEOUT,
            stream.read_exact(&mut header),
        )
        .await
        {
            // Idle for too long, or the module closed: done.
            Err(_) => return Ok(()),
            Ok(Err(err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Ok(Err(err)) => return Err(err.into()),
            Ok(Ok(_)) => {}
        }
        let len = usize::try_from(u32::from_be_bytes(header)).unwrap_or(usize::MAX);
        if len > MAX_REQUEST_SIZE {
            let response = DbConnectorResponse::err(connector_messages::request_too_large());
            write_frame(&mut stream, &response).await?;
            return Err(anyhow!(connector_messages::request_too_large()));
        }
        let mut body = vec![0u8; len];
        tokio::time::timeout(FRAME_BODY_TIMEOUT, stream.read_exact(&mut body))
            .await
            .map_err(|_| anyhow!("db connector frame incomplete"))??;
        let response = match serde_json::from_slice::<DbConnectorRequest>(&body) {
            Ok(request) => service.execute(request).await,
            Err(err) => DbConnectorResponse::err(connector_messages::request_invalid(err)),
        };
        write_frame(&mut stream, &response).await?;
    }
}

async fn write_frame<S>(stream: &mut S, response: &DbConnectorResponse) -> anyhow::Result<()>
where
    S: AsyncWrite + Unpin,
{
    let payload = serde_json::to_vec(response)?;
    let len = u32::try_from(payload.len()).map_err(|_| anyhow!("db connector answer too large"))?;
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(&payload);
    stream.write_all(&frame).await?;
    stream.flush().await?;
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

#[cfg(test)]
#[path = "../../../tests/unit/infra/db/connector_tests.rs"]
mod tests;
