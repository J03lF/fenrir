use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::dev_agent::config::DevAgentConfig;

pub async fn run(config_path: PathBuf) -> Result<()> {
    let data = fs::read_to_string(&config_path).await.with_context(|| {
        format!(
            "failed to read dev agent config at {}",
            config_path.display()
        )
    })?;
    let config: DevAgentConfig = serde_json::from_str(&data)
        .with_context(|| format!("invalid dev agent config at {}", config_path.display()))?;
    run_with_config(config).await
}

async fn run_with_config(config: DevAgentConfig) -> Result<()> {
    let logger = LogWriter::new(&config.log_path)
        .await
        .context("failed to open dev agent log file")?;
    logger
        .write_line("agent", &format!("launching {}", config.command_display))
        .await?;

    let shutdown = Shutdown::new();
    let shutdown_listener = shutdown.clone();
    tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        shutdown_listener.trigger();
    });

    enum Outcome {
        Exited(std::process::ExitStatus),
        Shutdown,
    }

    loop {
        let mut child = spawn_child(&config)?;
        let mut log_tasks: Vec<JoinHandle<()>> = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            log_tasks.push(spawn_forward(stdout, logger.clone(), "stdout"));
        }
        if let Some(stderr) = child.stderr.take() {
            log_tasks.push(spawn_forward(stderr, logger.clone(), "stderr"));
        }

        let outcome = tokio::select! {
            status = child.wait() => Outcome::Exited(status.context("dev process wait failed")?),
            _ = shutdown.wait() => Outcome::Shutdown,
        };

        match outcome {
            Outcome::Exited(status) => {
                for task in log_tasks {
                    let _ = task.await;
                }
                logger
                    .write_line("agent", &format!("process exited with {status}"))
                    .await?;
                if !config.auto_restart {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Outcome::Shutdown => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                for task in log_tasks {
                    let _ = task.await;
                }
                break;
            }
        }
    }

    logger.write_line("agent", "stopped").await?;

    Ok(())
}

fn spawn_child(config: &DevAgentConfig) -> Result<tokio::process::Child> {
    if config.command.is_empty() {
        return Err(anyhow!("dev agent command is empty"));
    }
    let mut cmd = Command::new(&config.command[0]);
    for arg in &config.command[1..] {
        cmd.arg(arg);
    }
    cmd.current_dir(&config.workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .envs(&config.env);
    cmd.spawn().context("failed to spawn dev process")
}

fn spawn_forward<R>(reader: R, logger: LogWriter, label: &'static str) -> JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        if let Err(err) = forward_stream(reader, logger, label).await {
            eprintln!("dev agent logging failed: {err}");
        }
    })
}

async fn forward_stream<R>(reader: R, logger: LogWriter, label: &'static str) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        logger.write_line(label, &line).await?;
    }
    Ok(())
}

#[derive(Clone)]
struct LogWriter {
    file: Arc<Mutex<tokio::fs::File>>,
}

impl LogWriter {
    async fn new(path: &PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        Ok(Self {
            file: Arc::new(Mutex::new(file)),
        })
    }

    async fn write_line(&self, label: &str, line: &str) -> Result<()> {
        let timestamp = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "unknown".to_string());
        let mut guard = self.file.lock().await;
        guard
            .write_all(format!("[{timestamp}] [{label}] {line}\n").as_bytes())
            .await?;
        guard.flush().await?;
        Ok(())
    }
}

#[derive(Clone)]
struct Shutdown {
    notify: Arc<tokio::sync::Notify>,
}

impl Shutdown {
    fn new() -> Self {
        Self {
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    async fn wait(&self) {
        self.notify.notified().await;
    }

    fn trigger(&self) {
        self.notify.notify_waiters();
    }
}

async fn wait_for_shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");

    #[cfg(unix)]
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate.recv() => {},
    }

    #[cfg(not(unix))]
    {
        let _ = ctrl_c.await;
    }
}
