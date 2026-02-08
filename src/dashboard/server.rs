use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;

use super::routes;
use super::state::DashboardState;
use super::websocket;

/// Start the Axum web dashboard server on the given port.
///
/// This runs as a background tokio task alongside the REPL.
pub async fn start_dashboard(state: Arc<DashboardState>, port: u16) -> anyhow::Result<()> {
    let app = Router::new()
        // HTML pages
        .route("/", get(routes::index))
        .route("/code/:filename", get(routes::view_code))
        // JSON API endpoints
        .route("/api/history", get(routes::get_history))
        .route("/api/stats", get(routes::get_stats))
        .route("/api/containers", get(routes::get_containers))
        .route("/api/generate", post(routes::generate_code))
        // HTMX HTML partials
        .route("/api/history/html", get(routes::get_history_html))
        .route("/api/stats/html", get(routes::get_stats_html))
        .route("/api/containers/html", get(routes::get_containers_html))
        // WebSocket for real-time logs
        .route("/api/logs", get(websocket::ws_handler))
        .with_state(state);

    let addr = format!("127.0.0.1:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    axum::serve(listener, app).await?;
    Ok(())
}
