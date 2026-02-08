use askama::Template;

use crate::config::AppConfig;
use crate::logger::SessionMetrics;
use super::routes::ContainerInfo;
use super::state::ScriptEntry;

// ── Askama Templates ─────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "index.html")]
pub struct IndexTemplate<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    pub docker_enabled: bool,
    pub venv_enabled: bool,
    pub dashboard_port: u16,
    pub scripts: &'a [ScriptEntry],
    pub total_requests: usize,
    pub successful_executions: usize,
    pub failed_executions: usize,
    pub api_errors: usize,
    pub success_rate: f64,
    pub last_code: &'a str,
}

#[derive(Template)]
#[template(path = "partials/history.html")]
pub struct HistoryTemplate<'a> {
    pub scripts: &'a [ScriptEntry],
}

#[derive(Template)]
#[template(path = "partials/code_viewer.html")]
pub struct CodeViewerTemplate<'a> {
    pub code: &'a str,
}

#[derive(Template)]
#[template(path = "partials/stats.html")]
pub struct StatsTemplate {
    pub total_requests: usize,
    pub successful_executions: usize,
    pub failed_executions: usize,
    pub api_errors: usize,
    pub success_rate: f64,
}

#[derive(Template)]
#[template(path = "partials/containers.html")]
pub struct ContainersTemplate<'a> {
    pub containers: &'a [ContainerInfo],
}

// ── Render helpers (called from routes.rs) ───────────────────────────

pub fn render_index(
    config: &AppConfig,
    scripts: &[ScriptEntry],
    metrics: &SessionMetrics,
    last_code: &str,
) -> axum::response::Html<String> {
    let template = IndexTemplate {
        provider: &config.provider,
        model: &config.model,
        docker_enabled: config.use_docker,
        venv_enabled: config.use_venv,
        dashboard_port: config.dashboard_port,
        scripts,
        total_requests: metrics.total_requests,
        successful_executions: metrics.successful_executions,
        failed_executions: metrics.failed_executions,
        api_errors: metrics.api_errors,
        success_rate: metrics.success_rate(),
        last_code,
    };
    axum::response::Html(template.render().unwrap_or_else(|e| {
        let msg = e.to_string().replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        format!("<h1>Template error: {}</h1>", msg)
    }))
}

pub fn render_history(scripts: &[ScriptEntry]) -> String {
    let template = HistoryTemplate { scripts };
    template.render().unwrap_or_default()
}

pub fn render_code_block(code: &str) -> String {
    let template = CodeViewerTemplate { code };
    template.render().unwrap_or_default()
}

pub fn render_stats(
    total_requests: usize,
    successful_executions: usize,
    failed_executions: usize,
    api_errors: usize,
    success_rate: f64,
) -> String {
    let template = StatsTemplate {
        total_requests,
        successful_executions,
        failed_executions,
        api_errors,
        success_rate,
    };
    template.render().unwrap_or_default()
}

pub fn render_containers(containers: &[ContainerInfo]) -> String {
    let template = ContainersTemplate { containers };
    template.render().unwrap_or_default()
}
