use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

use axum::{
    Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderValue, Method},
    response::IntoResponse,
    routing::get,
};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

use tokio::sync::broadcast;

use crate::{app_state::AppState, studio};

async fn handle_client(
    mut socket: WebSocket,
    ws_tx: broadcast::Sender<String>,
    last_hr_json: Arc<Mutex<Option<String>>>,
) {
    let snapshot = last_hr_json.lock().ok().and_then(|g| g.clone());
    if let Some(snapshot) = snapshot {
        let _ = socket.send(Message::Text(snapshot.into())).await;
    }
    let mut rx = ws_tx.subscribe();
    loop {
        match rx.recv().await {
            Ok(msg) => {
                if socket.send(Message::Text(msg.into())).await.is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn ws_handler(ws: WebSocketUpgrade, State(app): State<AppState>) -> impl IntoResponse {
    let ws_tx = app.ws_tx.clone();
    let last = app.last_hr_json.clone();
    ws.on_upgrade(move |socket| handle_client(socket, ws_tx, last))
}

fn studio_bind_ip() -> anyhow::Result<IpAddr> {
    let bind = std::env::var("OPENWHOOP_STUDIO_BIND")
        .map_err(|_| anyhow::anyhow!("OPENWHOOP_STUDIO_BIND must be set (e.g., 0.0.0.0 for network access, 127.0.0.1 for localhost only)"))?;
    bind.parse()
        .map_err(|e| anyhow::anyhow!("Invalid OPENWHOOP_STUDIO_BIND IP: {}", e))
}

/// When set (comma-separated `https://…` origins), enables CORS so a static UI
/// (e.g. GitHub Pages) can call `/api/*`. WebSocket browsers still connect to
/// your Studio URL directly (`wss://…/ws`).
fn cors_layer_from_env() -> Option<CorsLayer> {
    let raw = std::env::var("OPENWHOOP_CORS_ORIGIN").ok()?;
    let origins: Vec<HeaderValue> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| HeaderValue::from_str(s).ok())
        .collect();
    if origins.is_empty() {
        return None;
    }
    Some(
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::OPTIONS])
            .allow_headers(Any),
    )
}

/// Runs the thin live-server proxy.
/// Serves device control API endpoints and optional WebSocket for live HR streaming.
/// All scheduling logic moved to the AWS/scheduler side.
pub async fn run(app: AppState, port: u16) -> anyhow::Result<()> {
    let mut router = Router::new()
        .merge(studio::api_routes())
        .route("/ws", get(ws_handler))
        .route("/health", get(|| async { "ok" }))
        .with_state(app);

    if let Some(cors) = cors_layer_from_env() {
        router = router.layer(cors);
    }

    let bind_ip = studio_bind_ip()?;
    let addr = SocketAddr::from((bind_ip, port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    log::info!("Studio dashboard: http://{bind_ip}:{port}/");
    axum::serve(listener, router).await?;
    Ok(())
}
