use axum::{
    routing::{delete, get, post, put},
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
        // Execution
        .route("/api/execute", post(routes::execute_code))
        .route("/api/execute/kill", post(routes::kill_execution))
        .route("/api/execute/input", post(routes::send_input))
        // Lint & Security
        .route("/api/lint", post(routes::lint_code))
        .route("/api/security", post(routes::security_check_code))
        // Session management
        .route("/api/sessions", get(routes::list_sessions))
        .route("/api/sessions", post(routes::create_session))
        .route("/api/sessions/:id", get(routes::get_session))
        .route("/api/sessions/:id", delete(routes::delete_session))
        .route("/api/sessions/:id/active", put(routes::set_active_session))
        // Model selection & settings
        .route("/api/models", get(routes::get_models))
        .route("/api/settings", get(routes::get_settings))
        .route("/api/settings", post(routes::update_settings))
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
