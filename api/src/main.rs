mod agent;
mod config;
mod tools;

use agent::{anthropic::AnthropicBackend, AgentConfig, AgentEvent};
use axum::{
    extract::State,
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    routing::{get, post},
    Json, Router,
};
use deadpool_postgres::{Config as PgConfig, Runtime};
use futures::stream::Stream;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{convert::Infallible, sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tokio_postgres::NoTls;
use tower_http::cors::CorsLayer;

#[derive(Clone)]
struct AppState {
    tools: tools::Tools,
    cfg: config::Config,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,map_api=debug".into()),
        )
        .init();

    let cfg = config::Config::from_env()?;

    let mut pg = PgConfig::new();
    pg.host = Some(cfg.pg_host.clone());
    pg.port = Some(cfg.pg_port);
    pg.user = Some(cfg.pg_user.clone());
    pg.password = Some(cfg.pg_password.clone());
    pg.dbname = Some(cfg.pg_db.clone());
    let pool = match pg.create_pool(Some(Runtime::Tokio1), NoTls) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::error!("postgis pool could not be created: {e}");
            None
        }
    };

    let http = reqwest::Client::builder()
        // Long, because a single agent turn can legitimately run for minutes at
        // high effort. Streaming keeps the connection alive underneath this.
        .timeout(Duration::from_secs(900))
        .build()?;

    let state = AppState {
        tools: tools::Tools { cfg: cfg.clone(), http, pool },
        cfg: cfg.clone(),
    };

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/health", get(health))
        .route("/api/chat", post(chat))
        .route("/api/tool", post(run_tool))
        .layer(CorsLayer::permissive())
        .with_state(Arc::new(state));

    let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
    tracing::info!("map-api listening on {}", cfg.bind);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "model": st.cfg.model,
        "anthropic_key_present": !st.cfg.anthropic_api_key.is_empty(),
        "postgis": st.tools.pool.is_some(),
    }))
}

#[derive(Deserialize)]
struct ToolRequest {
    name: String,
    input: Value,
}

/// Run one tool directly, bypassing the model.
///
/// This is design spec §13's first test layer: the tool functions exercised against
/// real Photon, real PostGIS and real GraphHopper, with no mocking and no API key.
/// It reads public OSM data and holds no secrets, which is why it can sit on the
/// same public prefix as /api/chat.
async fn run_tool(
    State(st): State<Arc<AppState>>,
    Json(req): Json<ToolRequest>,
) -> impl IntoResponse {
    let outcome = st.tools.dispatch(&req.name, &req.input).await;
    let parsed: Value =
        serde_json::from_str(&outcome.content).unwrap_or_else(|_| json!(outcome.content));
    let status = if outcome.is_error { StatusCode::BAD_REQUEST } else { StatusCode::OK };
    (status, Json(json!({ "ok": !outcome.is_error, "result": parsed })))
}

#[derive(Deserialize)]
struct ChatRequest {
    /// A single new user message (the simple case).
    message: Option<String>,
    /// Or the full conversation, so the browser can carry history across turns.
    messages: Option<Vec<Value>>,
}

async fn chat(
    State(st): State<Arc<AppState>>,
    Json(req): Json<ChatRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    if st.cfg.anthropic_api_key.is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "ANTHROPIC_API_KEY is not configured on the server".into(),
        ));
    }

    let messages = match (req.messages, req.message) {
        (Some(m), _) if !m.is_empty() => m,
        (_, Some(text)) if !text.trim().is_empty() => {
            vec![json!({ "role": "user", "content": text })]
        }
        _ => return Err((StatusCode::BAD_REQUEST, "send `message` or `messages`".into())),
    };

    let (tx, rx) = mpsc::channel::<AgentEvent>(256);

    let backend = AnthropicBackend {
        http: st.tools.http.clone(),
        api_key: st.cfg.anthropic_api_key.clone(),
        base_url: st.cfg.anthropic_base.clone(),
    };
    let agent_cfg = AgentConfig {
        model: st.cfg.model.clone(),
        effort: st.cfg.effort.clone(),
        task_budget: st.cfg.task_budget,
        max_tokens: st.cfg.max_tokens,
        max_iterations: st.cfg.max_iterations,
    };
    let tools = st.tools.clone();

    tokio::spawn(async move {
        agent::run(&backend, &tools, &agent_cfg, messages, tx).await;
    });

    let stream = async_stream::stream(rx);
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

/// Adapt the agent's event channel into an SSE body.
mod async_stream {
    use super::*;
    use futures::stream::unfold;

    pub fn stream(
        rx: mpsc::Receiver<AgentEvent>,
    ) -> impl Stream<Item = Result<Event, Infallible>> {
        unfold(rx, |mut rx| async move {
            let ev = rx.recv().await?;
            let data = serde_json::to_string(&ev).unwrap_or_else(|_| "{}".into());
            Some((Ok(Event::default().data(data)), rx))
        })
    }
}
