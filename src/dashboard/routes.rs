use axum::{
    extract::State,
    response::{Html, IntoResponse, Json},
    Form,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::state::{DashboardState, ExecutionEvent, ScriptEntry};
use super::templates;
use crate::api::{self, Message};
use crate::utils::extract_python_code;

// ── GET / — main dashboard page ──────────────────────────────────────

pub async fn index(State(state): State<Arc<DashboardState>>) -> impl IntoResponse {
    let scripts = list_scripts_from_dir(&state.config.generated_dir).await;
    let metrics = state.metrics.read().await;
    let last_code = state.last_generated_code.read().await;
    templates::render_index(
        &state.config,
        &scripts,
        &metrics,
        &last_code,
    )
}

// ── GET /api/history — JSON list of generated scripts ────────────────

pub async fn get_history(State(state): State<Arc<DashboardState>>) -> impl IntoResponse {
    let scripts = list_scripts_from_dir(&state.config.generated_dir).await;
    Json(scripts)
}

// ── GET /api/history/html — HTML partial for HTMX swap ──────────────

pub async fn get_history_html(State(state): State<Arc<DashboardState>>) -> impl IntoResponse {
    let scripts = list_scripts_from_dir(&state.config.generated_dir).await;
    Html(templates::render_history(&scripts))
}

// ── GET /api/stats — session metrics as JSON ─────────────────────────

#[derive(Serialize)]
pub struct StatsResponse {
    pub total_requests: usize,
    pub successful_executions: usize,
    pub failed_executions: usize,
    pub api_errors: usize,
    pub success_rate: f64,
}

pub async fn get_stats(State(state): State<Arc<DashboardState>>) -> impl IntoResponse {
    let m = state.metrics.read().await;
    Json(StatsResponse {
        total_requests: m.total_requests,
        successful_executions: m.successful_executions,
        failed_executions: m.failed_executions,
        api_errors: m.api_errors,
        success_rate: m.success_rate(),
    })
}

// ── GET /api/stats/html — HTML partial for HTMX ─────────────────────

pub async fn get_stats_html(State(state): State<Arc<DashboardState>>) -> impl IntoResponse {
    let m = state.metrics.read().await;
    Html(templates::render_stats(
        m.total_requests,
        m.successful_executions,
        m.failed_executions,
        m.api_errors,
        m.success_rate(),
    ))
}

// ── POST /api/generate — accept prompt, call LLM, return HTML fragment

#[derive(Deserialize)]
pub struct GenerateRequest {
    pub prompt: String,
}

pub async fn generate_code(
    State(state): State<Arc<DashboardState>>,
    Form(req): Form<GenerateRequest>,
) -> impl IntoResponse {
    if req.prompt.trim().is_empty() {
        return Html(r#"<div class="text-yellow-400">Please enter a prompt.</div>"#.to_string());
    }

    // Add user message and snapshot history atomically to prevent race conditions
    let messages = {
        let mut history = state.conversation_history.write().await;
        history.push(Message {
            role: "user".to_string(),
            content: req.prompt.clone(),
        });
        history.clone()
    };

    // Call the LLM
    let result = api::generate_code_with_history(messages, &state.config).await;

    match result {
        Ok(raw_response) => {
            let code = extract_python_code(&raw_response);

            // Write the script to disk
            let script_path = match state.executor.write_script(&code) {
                Ok(p) => p.display().to_string(),
                Err(e) => {
                    return Html(format!(
                        r#"<div class="p-4 bg-red-900/30 border border-red-700 rounded text-red-300"><strong>Error writing script:</strong> {}</div>"#,
                        html_escape(&e.to_string())
                    ));
                }
            };

            // Update shared state
            {
                let mut last = state.last_generated_code.write().await;
                *last = code.clone();
            }
            {
                let mut history = state.conversation_history.write().await;
                history.push(Message {
                    role: "assistant".to_string(),
                    content: code.clone(),
                });
                // Enforce history limit
                let max = state.config.max_history_messages;
                while history.len() > max {
                    if history.len() >= 2 {
                        history.drain(..2);
                    } else {
                        history.remove(0);
                    }
                }
            }
            {
                let mut m = state.metrics.write().await;
                m.total_requests += 1;
            }

            // Broadcast event
            state.broadcast(ExecutionEvent::CodeGenerated {
                code: code.clone(),
                script_path,
            });

            Html(templates::render_code_block(&code))
        }
        Err(e) => {
            {
                let mut m = state.metrics.write().await;
                m.total_requests += 1;
                m.api_errors += 1;
            }
            Html(format!(
                r#"<div class="p-4 bg-red-900/30 border border-red-700 rounded text-red-300"><strong>Error:</strong> {}</div>"#,
                html_escape(&e.to_string())
            ))
        }
    }
}

// ── GET /api/containers — Docker container status ────────────────────

#[derive(Serialize)]
pub struct ContainerInfo {
    pub id: String,
    pub image: String,
    pub status: String,
    pub names: String,
}

pub async fn get_containers() -> impl IntoResponse {
    let containers = list_docker_containers().await;
    Json(containers)
}

// ── GET /api/containers/html — HTML partial for HTMX ────────────────

pub async fn get_containers_html() -> impl IntoResponse {
    let containers = list_docker_containers().await;
    Html(templates::render_containers(&containers))
}

// ── GET /code/:filename — view a specific script's source ───────────

pub async fn view_code(
    State(state): State<Arc<DashboardState>>,
    axum::extract::Path(filename): axum::extract::Path<String>,
) -> impl IntoResponse {
    let path = std::path::Path::new(&state.config.generated_dir).join(&filename);
    match tokio::fs::read_to_string(&path).await {
        Ok(code) => Html(templates::render_code_block(&code)),
        Err(_) => Html(format!(
            r#"<div class="text-red-400">Script not found: {}</div>"#,
            html_escape(&filename)
        )),
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

async fn list_scripts_from_dir(dir: &str) -> Vec<ScriptEntry> {
    let dir = dir.to_string();
    tokio::task::spawn_blocking(move || list_scripts_from_dir_sync(&dir))
        .await
        .unwrap_or_default()
}

fn list_scripts_from_dir_sync(dir: &str) -> Vec<ScriptEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut scripts: Vec<ScriptEntry> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|ext| ext == "py")
        })
        .map(|e| {
            let filename = e.file_name().to_string_lossy().to_string();
            let path = e.path().display().to_string();
            // Extract timestamp from filename: script_YYYYMMDD_HHMMSS.py
            let timestamp = filename
                .strip_prefix("script_")
                .and_then(|s| s.strip_suffix(".py"))
                .unwrap_or(&filename)
                .to_string();
            ScriptEntry {
                filename,
                path,
                timestamp,
            }
        })
        .collect();

    // Sort by filename descending (newest first)
    scripts.sort_by(|a, b| b.filename.cmp(&a.filename));
    scripts
}

async fn list_docker_containers() -> Vec<ContainerInfo> {
    tokio::task::spawn_blocking(list_docker_containers_sync)
        .await
        .unwrap_or_default()
}

fn list_docker_containers_sync() -> Vec<ContainerInfo> {
    let output = std::process::Command::new("docker")
        .args(["ps", "--filter", "ancestor=python-sandbox", "--format", "{{.ID}}\t{{.Image}}\t{{.Status}}\t{{.Names}}"])
        .output();

    let Ok(output) = output else {
        return Vec::new();
    };

    if !output.status.success() {
        return Vec::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 4 {
                Some(ContainerInfo {
                    id: parts[0].to_string(),
                    image: parts[1].to_string(),
                    status: parts[2].to_string(),
                    names: parts[3].to_string(),
                })
            } else {
                None
            }
        })
        .collect()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
