use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use fenrir::config;
use fenrir::config::AppConfig;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio_postgres::{types::ToSql, Client, NoTls, Row};
use tracing::{info, warn};
use uuid::Uuid;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = config::load()?;
    init_tracing(&cfg);

    let client = init_db(&cfg).await?;
    let state = ApiState {
        db: Arc::new(client),
    };

    let router = Router::new()
        .route("/api/tickets", get(list_tickets).post(create_ticket))
        .route("/api/tickets/:id", get(get_ticket))
        .with_state(state);

    let addr: SocketAddr = "0.0.0.0:8081".parse()?;
    let listener = TcpListener::bind(addr).await?;
    info!(%addr, "fenrir-api ready");
    axum::serve(listener, router).await?;
    Ok(())
}

fn init_tracing(cfg: &AppConfig) {
    let level = cfg
        .telemetry
        .tracing_level
        .parse()
        .unwrap_or(tracing::Level::INFO);
    let _ = tracing_subscriber::fmt()
        .with_thread_names(true)
        .with_target(false)
        .with_max_level(level)
        .try_init();
}

async fn init_db(cfg: &AppConfig) -> anyhow::Result<Client> {
    let settings = cfg
        .db
        .connections
        .postgres
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("db.connections.postgres not configured"))?;
    let uri = settings.resolve_uri("fenrir_api.postgres")?;
    let (client, connection) = tokio_postgres::connect(&uri, NoTls).await?;
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            warn!(error = %err, "postgres connection dropped");
        }
    });
    Ok(client)
}

#[derive(Clone)]
struct ApiState {
    db: Arc<Client>,
}

#[derive(Debug, Deserialize, Default)]
struct TicketQuery {
    status: Option<String>,
    reporter: Option<String>,
    assignee: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TicketCreateRequest {
    title: String,
    description: String,
    #[serde(default = "default_priority")]
    priority: String,
    reporter_id: String,
    assignee_id: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

fn default_priority() -> String {
    "medium".to_string()
}

#[derive(Debug, Serialize)]
struct TicketResponse {
    id: String,
    title: String,
    description: String,
    status: String,
    priority: String,
    reporter_id: String,
    assignee_id: Option<String>,
    tags: Vec<String>,
}

#[derive(Debug, Serialize)]
struct TicketListResponse {
    tickets: Vec<TicketResponse>,
}

#[derive(Debug, Serialize)]
struct TicketCreateResponse {
    ticket: TicketResponse,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    code: String,
    message: String,
}

impl ErrorResponse {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

async fn list_tickets(
    State(state): State<ApiState>,
    Query(query): Query<TicketQuery>,
) -> impl IntoResponse {
    let mut clauses = Vec::new();
    let mut params: Vec<String> = Vec::new();
    if let Some(status) = &query.status {
        clauses.push(format!("status = ${}", params.len() + 1));
        params.push(status.to_string());
    }
    if let Some(reporter) = &query.reporter {
        clauses.push(format!("reporter_id = ${}", params.len() + 1));
        params.push(reporter.to_string());
    }
    if let Some(assignee) = &query.assignee {
        clauses.push(format!("assignee_id = ${}", params.len() + 1));
        params.push(assignee.to_string());
    }
    let filter = if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    };
    let sql = format!(
        "SELECT id, title, description, status, priority, reporter_id, assignee_id, COALESCE(tags, ARRAY[]::text[]) as tags FROM tickets {filter} ORDER BY created_at DESC LIMIT 100"
    );
    let prepared: Vec<&(dyn ToSql + Sync)> = params
        .iter()
        .map(|value| value as &(dyn ToSql + Sync))
        .collect();
    match state.db.query(&sql, &prepared).await {
        Ok(rows) => {
            let tickets = rows.iter().map(row_to_ticket).collect();
            Json(TicketListResponse { tickets }).into_response()
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse::new("tickets_query_failed", err.to_string())),
        )
            .into_response(),
    }
}

async fn get_ticket(State(state): State<ApiState>, Path(id): Path<String>) -> impl IntoResponse {
    let sql = "SELECT id, title, description, status, priority, reporter_id, assignee_id, COALESCE(tags, ARRAY[]::text[]) as tags FROM tickets WHERE id = $1";
    match state.db.query_opt(sql, &[&id]).await {
        Ok(Some(row)) => Json(row_to_ticket(&row)).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse::new(
                "ticket_not_found",
                "Ticket wurde nicht gefunden",
            )),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse::new("ticket_query_failed", err.to_string())),
        )
            .into_response(),
    }
}

async fn create_ticket(
    State(state): State<ApiState>,
    Json(payload): Json<TicketCreateRequest>,
) -> impl IntoResponse {
    let sql = "INSERT INTO tickets (id, title, description, status, priority, reporter_id, assignee_id, tags) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id, title, description, status, priority, reporter_id, assignee_id, COALESCE(tags, ARRAY[]::text[]) as tags";
    let ticket_id = Uuid::new_v4();
    let tags: Vec<&str> = payload.tags.iter().map(String::as_str).collect();
    match state
        .db
        .query_one(
            sql,
            &[
                &ticket_id,
                &payload.title,
                &payload.description,
                &"open",
                &payload.priority,
                &payload.reporter_id,
                &payload.assignee_id,
                &tags,
            ],
        )
        .await
    {
        Ok(row) => (
            StatusCode::CREATED,
            Json(TicketCreateResponse {
                ticket: row_to_ticket(&row),
            }),
        )
            .into_response(),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::new("ticket_create_failed", err.to_string())),
        )
            .into_response(),
    }
}

fn row_to_ticket(row: &Row) -> TicketResponse {
    let assignee: Option<Uuid> = row.get("assignee_id");
    let tags: Vec<String> = row.get("tags");
    TicketResponse {
        id: row.get::<_, Uuid>("id").to_string(),
        title: row.get("title"),
        description: row.get("description"),
        status: row.get::<_, String>("status"),
        priority: row.get::<_, String>("priority"),
        reporter_id: row.get::<_, Uuid>("reporter_id").to_string(),
        assignee_id: assignee.map(|v| v.to_string()),
        tags,
    }
}
