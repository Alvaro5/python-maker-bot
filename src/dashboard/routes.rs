use axum::{
    extract::State,
    response::{Html, IntoResponse, Json},
    Form,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::state::{ChatSession, DashboardState, ExecutionEvent, RuntimeSettings, ScriptEntry};
use super::templates;
use crate::api::{self, Message};
use crate::utils::extract_python_code;

use std::io::{BufRead, BufReader, Write};
use wait_timeout::ChildExt;

// ── GET / — main dashboard page ──────────────────────────────────────

pub async fn index(State(state): State<Arc<DashboardState>>) -> impl IntoResponse {
    let scripts = list_scripts_from_dir(&state.config.generated_dir).await;
    let metrics = state.metrics.read().await;
    let sessions = state.sessions.read().await;
    let active_id = state.active_session_id.read().await;
    let settings = state.runtime_settings.read().await;

    // Collect session list for the sidebar
    let mut session_list: Vec<SessionListEntry> = sessions
        .values()
        .map(|s| SessionListEntry {
            id: s.id.clone(),
            name: s.name.clone(),
            message_count: s.messages.len(),
            created_at: s.created_at.clone(),
        })
        .collect();
    session_list.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    // Get messages for the active session
    let active_messages: Vec<ChatMessageView> = sessions
        .get(&*active_id)
        .map(|s| {
            s.messages
                .iter()
                .map(|m| ChatMessageView {
                    role: m.role.clone(),
                    content: m.content.clone(),
                    is_code: m.role == "assistant",
                })
                .collect()
        })
        .unwrap_or_default();

    let last_code = sessions
        .get(&*active_id)
        .map(|s| s.last_generated_code.clone())
        .unwrap_or_default();

    templates::render_index(
        &settings,
        &scripts,
        &metrics,
        &last_code,
        &session_list,
        &active_id,
        &active_messages,
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

// ── POST /api/generate — accept prompt, call LLM, return JSON ────────

#[derive(Deserialize)]
pub struct GenerateRequest {
    pub prompt: String,
    #[serde(default)]
    pub session_id: String,
}

/// Response returned to the chat UI after code generation.
#[derive(Serialize)]
pub struct GenerateResponse {
    pub success: bool,
    pub code: String,
    pub script_path: String,
    pub error: String,
}

pub async fn generate_code(
    State(state): State<Arc<DashboardState>>,
    Form(req): Form<GenerateRequest>,
) -> impl IntoResponse {
    if req.prompt.trim().is_empty() {
        return Json(GenerateResponse {
            success: false,
            code: String::new(),
            script_path: String::new(),
            error: "Please enter a prompt.".to_string(),
        });
    }

    // Resolve session ID — fall back to active session if not provided
    let session_id = if req.session_id.is_empty() {
        state.active_session_id.read().await.clone()
    } else {
        req.session_id.clone()
    };

    // Add user message to session and snapshot history for the LLM call
    let messages = {
        let mut sessions = state.sessions.write().await;
        if let Some(session) = sessions.get_mut(&session_id) {
            session.messages.push(Message {
                role: "user".to_string(),
                content: req.prompt.clone(),
            });
            // Auto-rename session from "New Chat" after first user message
            if session.name == "New Chat" && session.messages.len() <= 2 {
                let name: String = req.prompt.chars().take(40).collect();
                session.name = if req.prompt.len() > 40 {
                    format!("{}...", name)
                } else {
                    name
                };
            }
            session.messages.clone()
        } else {
            return Json(GenerateResponse {
                success: false,
                code: String::new(),
                script_path: String::new(),
                error: "Session not found.".to_string(),
            });
        }
    };

    // Build ephemeral config from runtime settings
    let effective_config = {
        let settings = state.runtime_settings.read().await;
        settings.to_app_config(&state.config)
    };

    // Call the LLM
    let result = api::generate_code_with_history(messages, &effective_config).await;

    match result {
        Ok(raw_response) => {
            let code = extract_python_code(&raw_response);

            // Write the script to disk
            let script_path = match state.executor.write_script(&code) {
                Ok(p) => p.display().to_string(),
                Err(e) => {
                    return Json(GenerateResponse {
                        success: false,
                        code: String::new(),
                        script_path: String::new(),
                        error: format!("Error writing script: {}", e),
                    });
                }
            };

            // Update session state
            {
                let mut sessions = state.sessions.write().await;
                if let Some(session) = sessions.get_mut(&session_id) {
                    session.messages.push(Message {
                        role: "assistant".to_string(),
                        content: code.clone(),
                    });
                    session.last_generated_code = code.clone();
                    // Enforce history limit
                    let max = effective_config.max_history_messages;
                    while session.messages.len() > max {
                        if session.messages.len() >= 2 {
                            session.messages.drain(..2);
                        } else {
                            session.messages.remove(0);
                        }
                    }
                }
            }

            // Also update legacy flat state for REPL sync
            {
                let mut last = state.last_generated_code.write().await;
                *last = code.clone();
            }
            {
                let mut history = state.conversation_history.write().await;
                history.push(Message {
                    role: "user".to_string(),
                    content: req.prompt.clone(),
                });
                history.push(Message {
                    role: "assistant".to_string(),
                    content: code.clone(),
                });
                let max = effective_config.max_history_messages;
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
                script_path: script_path.clone(),
            });

            Json(GenerateResponse {
                success: true,
                code,
                script_path,
                error: String::new(),
            })
        }
        Err(e) => {
            {
                let mut m = state.metrics.write().await;
                m.total_requests += 1;
                m.api_errors += 1;
            }
            Json(GenerateResponse {
                success: false,
                code: String::new(),
                script_path: String::new(),
                error: e.to_string(),
            })
        }
    }
}

// ══════════════════════════════════════════════════════════════════════
//  Code Execution (streaming via WebSocket)
// ══════════════════════════════════════════════════════════════════════

#[derive(Deserialize)]
pub struct ExecuteRequest {
    pub code: String,
}

#[derive(Serialize)]
pub struct ExecuteAccepted {
    pub status: String,
    pub script_path: String,
}

/// Accept code, spawn execution in background, stream output via WebSocket.
/// Returns 202 Accepted immediately.
pub async fn execute_code(
    State(state): State<Arc<DashboardState>>,
    Json(req): Json<ExecuteRequest>,
) -> impl IntoResponse {
    if req.code.trim().is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(ExecuteAccepted {
                status: "error".to_string(),
                script_path: String::new(),
            }),
        );
    }

    // Write script to disk
    let script_path = match state.executor.write_script(&req.code) {
        Ok(p) => p,
        Err(e) => {
            state.broadcast(ExecutionEvent::LogLine {
                timestamp: now_hms(),
                stream: "stderr".to_string(),
                content: format!("Error writing script: {}", e),
            });
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(ExecuteAccepted {
                    status: "error".to_string(),
                    script_path: String::new(),
                }),
            );
        }
    };

    let script_path_str = script_path.display().to_string();

    // Read runtime settings
    let settings = state.runtime_settings.read().await.clone();

    // Spawn background execution task
    let execution_state = Arc::clone(&state);
    let exec_script_path = script_path.clone();
    let exec_script_path_str = script_path_str.clone();
    let code_for_deps = req.code.clone();

    tokio::task::spawn_blocking(move || {
        execute_script_with_streaming(
            execution_state,
            exec_script_path,
            &exec_script_path_str,
            &code_for_deps,
            &settings,
        );
    });

    (
        axum::http::StatusCode::ACCEPTED,
        Json(ExecuteAccepted {
            status: "accepted".to_string(),
            script_path: script_path_str,
        }),
    )
}

/// Synchronous function that runs the full execution pipeline with real-time
/// output streaming via broadcast events.
fn execute_script_with_streaming(
    state: Arc<DashboardState>,
    script_path: std::path::PathBuf,
    script_path_str: &str,
    code: &str,
    settings: &RuntimeSettings,
) {
    // 1. Broadcast execution started
    state.broadcast(ExecutionEvent::ExecutionStarted {
        script_path: script_path_str.to_string(),
    });

    // 2. Syntax check
    state.broadcast(ExecutionEvent::LogLine {
        timestamp: now_hms(),
        stream: "info".to_string(),
        content: "Running syntax check...".to_string(),
    });

    if let Err(e) = state.executor.syntax_check(&script_path) {
        state.broadcast(ExecutionEvent::LogLine {
            timestamp: now_hms(),
            stream: "stderr".to_string(),
            content: format!("Syntax error: {}", e),
        });
        state.broadcast(ExecutionEvent::ExecutionCompleted {
            success: false,
            exit_code: None,
        });
        let mut m = state.metrics.blocking_write();
        m.failed_executions += 1;
        return;
    }

    state.broadcast(ExecutionEvent::LogLine {
        timestamp: now_hms(),
        stream: "info".to_string(),
        content: "Syntax check passed.".to_string(),
    });

    // 3. Lint check (if enabled)
    if settings.use_linting {
        state.broadcast(ExecutionEvent::LogLine {
            timestamp: now_hms(),
            stream: "info".to_string(),
            content: "Running lint check (ruff)...".to_string(),
        });

        match state.executor.lint_check(&script_path) {
            Ok(lint_result) => {
                let diag_text = lint_result
                    .diagnostics
                    .iter()
                    .map(|d| d.message.clone())
                    .collect::<Vec<_>>()
                    .join("\n");
                let summary = if lint_result.passed {
                    "Lint check passed.".to_string()
                } else {
                    format!("Lint: {}", lint_result.summary)
                };
                state.broadcast(ExecutionEvent::LogLine {
                    timestamp: now_hms(),
                    stream: if lint_result.has_errors { "stderr" } else { "info" }.to_string(),
                    content: summary,
                });
                state.broadcast(ExecutionEvent::LintCompleted {
                    passed: lint_result.passed,
                    diagnostics: diag_text,
                });
            }
            Err(e) => {
                state.broadcast(ExecutionEvent::LogLine {
                    timestamp: now_hms(),
                    stream: "stderr".to_string(),
                    content: format!("Lint check error: {}", e),
                });
            }
        }
    }

    // 4. Security check (if enabled)
    if settings.use_security_check {
        state.broadcast(ExecutionEvent::LogLine {
            timestamp: now_hms(),
            stream: "info".to_string(),
            content: "Running security scan (bandit)...".to_string(),
        });

        match state.executor.security_check(&script_path) {
            Ok(sec_result) => {
                let diag_text = sec_result
                    .diagnostics
                    .iter()
                    .map(|d| d.message.clone())
                    .collect::<Vec<_>>()
                    .join("\n");
                let summary = if sec_result.passed {
                    "Security scan passed.".to_string()
                } else {
                    format!("Security: {}", sec_result.summary)
                };
                state.broadcast(ExecutionEvent::LogLine {
                    timestamp: now_hms(),
                    stream: if sec_result.has_high_severity {
                        "stderr"
                    } else {
                        "info"
                    }
                    .to_string(),
                    content: summary,
                });
                state.broadcast(ExecutionEvent::SecurityCompleted {
                    passed: sec_result.passed,
                    diagnostics: diag_text,
                });

                // Block on HIGH severity
                if sec_result.has_high_severity {
                    state.broadcast(ExecutionEvent::LogLine {
                        timestamp: now_hms(),
                        stream: "stderr".to_string(),
                        content: "Execution blocked: HIGH severity security finding.".to_string(),
                    });
                    state.broadcast(ExecutionEvent::ExecutionCompleted {
                        success: false,
                        exit_code: None,
                    });
                    let mut m = state.metrics.blocking_write();
                    m.failed_executions += 1;
                    return;
                }
            }
            Err(e) => {
                state.broadcast(ExecutionEvent::LogLine {
                    timestamp: now_hms(),
                    stream: "stderr".to_string(),
                    content: format!("Security scan error: {}", e),
                });
            }
        }
    }

    // 5. Detect and install dependencies
    let deps = state.executor.detect_dependencies(code);
    if !deps.is_empty() {
        state.broadcast(ExecutionEvent::LogLine {
            timestamp: now_hms(),
            stream: "info".to_string(),
            content: format!("Detected dependencies: {}", deps.join(", ")),
        });
    }

    // 6. Create venv if needed
    let venv_path = match state.executor.create_venv() {
        Ok(vp) => vp,
        Err(e) => {
            state.broadcast(ExecutionEvent::LogLine {
                timestamp: now_hms(),
                stream: "stderr".to_string(),
                content: format!("Venv creation failed: {}", e),
            });
            None
        }
    };

    if !deps.is_empty() {
        if let Err(e) = state
            .executor
            .install_packages(&deps, venv_path.as_deref())
        {
            state.broadcast(ExecutionEvent::LogLine {
                timestamp: now_hms(),
                stream: "stderr".to_string(),
                content: format!("Dependency install failed: {}", e),
            });
        }
    }

    // 7. Execute with real-time output streaming and interactive stdin support
    state.broadcast(ExecutionEvent::LogLine {
        timestamp: now_hms(),
        stream: "info".to_string(),
        content: "Executing script...".to_string(),
    });

    let timeout_secs = settings.execution_timeout_secs;

    match state.executor.spawn_piped(&script_path, venv_path.as_deref(), &deps) {
        Ok(mut child) => {
            // Store PID for kill support
            let child_pid = child.id();
            {
                let mut pid_lock = state.running_pid.blocking_lock();
                *pid_lock = Some(child_pid);
            }

            // Take stdin and store it in shared state for the web input endpoint
            let child_stdin = child.stdin.take();
            {
                let mut stdin_lock = state.running_stdin.blocking_lock();
                *stdin_lock = child_stdin;
            }

            // Take stdout and stderr for line-by-line streaming
            let child_stdout = child.stdout.take();
            let child_stderr = child.stderr.take();

            // Stream stdout in a separate thread
            let stdout_state = Arc::clone(&state);
            let stdout_handle = std::thread::spawn(move || {
                if let Some(stdout) = child_stdout {
                    let reader = BufReader::new(stdout);
                    for line in reader.lines() {
                        match line {
                            Ok(text) => {
                                stdout_state.broadcast(ExecutionEvent::LogLine {
                                    timestamp: now_hms(),
                                    stream: "stdout".to_string(),
                                    content: text,
                                });
                            }
                            Err(_) => break,
                        }
                    }
                }
            });

            // Stream stderr in a separate thread
            let stderr_state = Arc::clone(&state);
            let stderr_handle = std::thread::spawn(move || {
                if let Some(stderr) = child_stderr {
                    let reader = BufReader::new(stderr);
                    for line in reader.lines() {
                        match line {
                            Ok(text) => {
                                stderr_state.broadcast(ExecutionEvent::LogLine {
                                    timestamp: now_hms(),
                                    stream: "stderr".to_string(),
                                    content: text,
                                });
                            }
                            Err(_) => break,
                        }
                    }
                }
            });

            // Wait for the child process with optional timeout
            let exit_code = if timeout_secs > 0 {
                let timeout = std::time::Duration::from_secs(timeout_secs);
                match child.wait_timeout(timeout) {
                    Ok(Some(status)) => status.code(),
                    Ok(None) => {
                        // Timed out — kill the process
                        let _ = child.kill();
                        let _ = child.wait();
                        state.broadcast(ExecutionEvent::LogLine {
                            timestamp: now_hms(),
                            stream: "stderr".to_string(),
                            content: format!(
                                "Process timed out after {} seconds.",
                                timeout_secs
                            ),
                        });
                        None
                    }
                    Err(e) => {
                        state.broadcast(ExecutionEvent::LogLine {
                            timestamp: now_hms(),
                            stream: "stderr".to_string(),
                            content: format!("Error waiting for process: {}", e),
                        });
                        None
                    }
                }
            } else {
                // No timeout — blocking wait
                match child.wait() {
                    Ok(status) => status.code(),
                    Err(e) => {
                        state.broadcast(ExecutionEvent::LogLine {
                            timestamp: now_hms(),
                            stream: "stderr".to_string(),
                            content: format!("Error waiting for process: {}", e),
                        });
                        None
                    }
                }
            };

            // Wait for reader threads to finish
            let _ = stdout_handle.join();
            let _ = stderr_handle.join();

            // Clear PID and stdin from state
            {
                let mut pid_lock = state.running_pid.blocking_lock();
                *pid_lock = None;
            }
            {
                let mut stdin_lock = state.running_stdin.blocking_lock();
                *stdin_lock = None;
            }

            let success = exit_code == Some(0);
            state.broadcast(ExecutionEvent::ExecutionCompleted {
                success,
                exit_code,
            });

            let mut m = state.metrics.blocking_write();
            if success {
                m.successful_executions += 1;
            } else {
                m.failed_executions += 1;
            }
        }
        Err(e) => {
            state.broadcast(ExecutionEvent::LogLine {
                timestamp: now_hms(),
                stream: "stderr".to_string(),
                content: format!("Execution error: {}", e),
            });
            state.broadcast(ExecutionEvent::ExecutionCompleted {
                success: false,
                exit_code: None,
            });
            let mut m = state.metrics.blocking_write();
            m.failed_executions += 1;
        }
    }

    // Cleanup venv
    if let Some(vp) = venv_path {
        state.executor.cleanup_venv(&vp);
    }
}

// ── POST /api/execute/kill — kill running script ─────────────────────

pub async fn kill_execution(
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    let mut pid_lock = state.running_pid.lock().await;
    if let Some(pid) = pid_lock.take() {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .output();
        state.broadcast(ExecutionEvent::ExecutionKilled);
        Json(serde_json::json!({ "status": "killed", "pid": pid }))
    } else {
        Json(serde_json::json!({ "status": "no_process" }))
    }
}

// ── POST /api/execute/input — send stdin input to running script ─────

#[derive(Deserialize)]
pub struct SendInputRequest {
    pub input: String,
}

/// Write a line of text to the stdin of the currently running script.
pub async fn send_input(
    State(state): State<Arc<DashboardState>>,
    Json(req): Json<SendInputRequest>,
) -> impl IntoResponse {
    let mut stdin_lock = state.running_stdin.lock().await;
    if let Some(ref mut stdin) = *stdin_lock {
        let line = format!("{}\n", req.input);
        match stdin.write_all(line.as_bytes()) {
            Ok(()) => {
                let _ = stdin.flush();
                // Echo the input in the output panel so the user sees it
                state.broadcast(ExecutionEvent::LogLine {
                    timestamp: now_hms(),
                    stream: "stdin".to_string(),
                    content: req.input.clone(),
                });
                Json(serde_json::json!({ "status": "sent" }))
            }
            Err(e) => {
                Json(serde_json::json!({ "status": "error", "message": format!("Write failed: {}", e) }))
            }
        }
    } else {
        Json(serde_json::json!({ "status": "no_process", "message": "No running process to send input to" }))
    }
}

// ══════════════════════════════════════════════════════════════════════
//  Lint & Security (on-demand from dashboard)
// ══════════════════════════════════════════════════════════════════════

#[derive(Deserialize)]
pub struct CodePayload {
    pub code: String,
}

#[derive(Serialize)]
pub struct LintApiResponse {
    pub passed: bool,
    pub has_errors: bool,
    pub diagnostics: Vec<LintDiagnosticView>,
    pub summary: String,
}

#[derive(Serialize)]
pub struct LintDiagnosticView {
    pub message: String,
    pub severity: String,
}

pub async fn lint_code(
    State(state): State<Arc<DashboardState>>,
    Json(req): Json<CodePayload>,
) -> impl IntoResponse {
    let code = req.code.clone();
    let base_dir = state.executor.base_dir().to_path_buf();

    let result = tokio::task::spawn_blocking(move || {
        let tmp_path = base_dir.join("_lint_check_tmp.py");
        std::fs::write(&tmp_path, &code).map_err(|e| e.to_string())?;
        let r = crate::python_exec::CodeExecutor::lint_check_static(&tmp_path);
        let _ = std::fs::remove_file(&tmp_path);
        r.map_err(|e| e.to_string())
    })
    .await;

    match result {
        Ok(Ok(lint_result)) => Json(LintApiResponse {
            passed: lint_result.passed,
            has_errors: lint_result.has_errors,
            diagnostics: lint_result
                .diagnostics
                .iter()
                .map(|d| LintDiagnosticView {
                    message: d.message.clone(),
                    severity: match d.severity {
                        crate::python_exec::LintSeverity::Error => "error".to_string(),
                        crate::python_exec::LintSeverity::Warning => "warning".to_string(),
                    },
                })
                .collect(),
            summary: lint_result.summary,
        }),
        _ => Json(LintApiResponse {
            passed: false,
            has_errors: true,
            diagnostics: vec![LintDiagnosticView {
                message: "Lint check failed to run".to_string(),
                severity: "error".to_string(),
            }],
            summary: "Lint check failed".to_string(),
        }),
    }
}

#[derive(Serialize)]
pub struct SecurityApiResponse {
    pub passed: bool,
    pub has_high_severity: bool,
    pub diagnostics: Vec<SecurityDiagnosticView>,
    pub summary: String,
}

#[derive(Serialize)]
pub struct SecurityDiagnosticView {
    pub message: String,
    pub severity: String,
    pub confidence: String,
}

pub async fn security_check_code(
    State(state): State<Arc<DashboardState>>,
    Json(req): Json<CodePayload>,
) -> impl IntoResponse {
    let code = req.code.clone();
    let base_dir = state.executor.base_dir().to_path_buf();

    let result = tokio::task::spawn_blocking(move || {
        let tmp_path = base_dir.join("_security_check_tmp.py");
        std::fs::write(&tmp_path, &code).map_err(|e| e.to_string())?;
        let r = crate::python_exec::CodeExecutor::security_check_static(&tmp_path);
        let _ = std::fs::remove_file(&tmp_path);
        r.map_err(|e| e.to_string())
    })
    .await;

    match result {
        Ok(Ok(sec_result)) => Json(SecurityApiResponse {
            passed: sec_result.passed,
            has_high_severity: sec_result.has_high_severity,
            diagnostics: sec_result
                .diagnostics
                .iter()
                .map(|d| SecurityDiagnosticView {
                    message: d.message.clone(),
                    severity: d.severity.to_string(),
                    confidence: d.confidence.to_string(),
                })
                .collect(),
            summary: sec_result.summary,
        }),
        _ => Json(SecurityApiResponse {
            passed: false,
            has_high_severity: false,
            diagnostics: vec![SecurityDiagnosticView {
                message: "Security check failed to run".to_string(),
                severity: "error".to_string(),
                confidence: "N/A".to_string(),
            }],
            summary: "Security check failed".to_string(),
        }),
    }
}

// ══════════════════════════════════════════════════════════════════════
//  Session Management
// ══════════════════════════════════════════════════════════════════════

#[derive(Serialize)]
pub struct SessionListEntry {
    pub id: String,
    pub name: String,
    pub message_count: usize,
    pub created_at: String,
}

/// GET /api/sessions — list all sessions
pub async fn list_sessions(
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    let sessions = state.sessions.read().await;
    let active_id = state.active_session_id.read().await;

    let mut list: Vec<serde_json::Value> = sessions
        .values()
        .map(|s| {
            serde_json::json!({
                "id": s.id,
                "name": s.name,
                "message_count": s.messages.len(),
                "created_at": s.created_at,
                "active": s.id == *active_id,
            })
        })
        .collect();
    list.sort_by(|a, b| {
        b["created_at"]
            .as_str()
            .unwrap_or("")
            .cmp(a["created_at"].as_str().unwrap_or(""))
    });

    Json(list)
}

/// POST /api/sessions — create a new session
pub async fn create_session(
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    let new_id = uuid::Uuid::new_v4().to_string();
    let session = ChatSession {
        id: new_id.clone(),
        name: "New Chat".to_string(),
        messages: Vec::new(),
        last_generated_code: String::new(),
        created_at: chrono::Local::now()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
    };

    {
        let mut sessions = state.sessions.write().await;
        sessions.insert(new_id.clone(), session);
    }
    {
        let mut active = state.active_session_id.write().await;
        *active = new_id.clone();
    }

    Json(serde_json::json!({ "id": new_id, "status": "created" }))
}

/// DELETE /api/sessions/:id — delete a session
pub async fn delete_session(
    State(state): State<Arc<DashboardState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    let mut sessions = state.sessions.write().await;

    if sessions.len() <= 1 {
        return Json(
            serde_json::json!({ "status": "error", "message": "Cannot delete the last session" }),
        );
    }

    sessions.remove(&id);

    // If we deleted the active session, switch to another
    let mut active = state.active_session_id.write().await;
    if *active == id {
        if let Some(next_id) = sessions.keys().next() {
            *active = next_id.clone();
        }
    }

    Json(serde_json::json!({ "status": "deleted" }))
}

/// GET /api/sessions/:id — get full session with messages
pub async fn get_session(
    State(state): State<Arc<DashboardState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    let sessions = state.sessions.read().await;
    if let Some(session) = sessions.get(&id) {
        Json(serde_json::json!({
            "id": session.id,
            "name": session.name,
            "messages": session.messages,
            "last_generated_code": session.last_generated_code,
            "created_at": session.created_at,
        }))
    } else {
        Json(serde_json::json!({ "error": "Session not found" }))
    }
}

/// PUT /api/sessions/:id/active — set session as active
pub async fn set_active_session(
    State(state): State<Arc<DashboardState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    let sessions = state.sessions.read().await;
    if sessions.contains_key(&id) {
        drop(sessions);
        let mut active = state.active_session_id.write().await;
        *active = id.clone();
        Json(serde_json::json!({ "status": "ok", "active_session": id }))
    } else {
        Json(serde_json::json!({ "status": "error", "message": "Session not found" }))
    }
}

// ══════════════════════════════════════════════════════════════════════
//  Model Selection
// ══════════════════════════════════════════════════════════════════════

#[derive(Serialize)]
pub struct ModelsResponse {
    pub providers: Vec<ProviderModels>,
    pub current_provider: String,
    pub current_model: String,
}

#[derive(Serialize)]
pub struct ProviderModels {
    pub name: String,
    pub id: String,
    pub models: Vec<String>,
}

/// GET /api/models — return available models grouped by provider.
/// Fetches live model lists from HuggingFace and Ollama at runtime.
pub async fn get_models(
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    let settings = state.runtime_settings.read().await;
    let current_provider = settings.provider.clone();
    let current_model = settings.model.clone();
    drop(settings);

    // Fetch live model lists from HF and Ollama in parallel
    let (hf_models, ollama_models) =
        tokio::join!(fetch_hf_models(), fetch_ollama_models());

    let openai_models = vec![
        "gpt-4o".to_string(),
        "gpt-4o-mini".to_string(),
        "gpt-4-turbo".to_string(),
        "gpt-3.5-turbo".to_string(),
        "o3-mini".to_string(),
        "claude-3-5-sonnet-20241022".to_string(),
        "deepseek-chat".to_string(),
        "deepseek-coder".to_string(),
    ];

    Json(ModelsResponse {
        providers: vec![
            ProviderModels {
                name: "HuggingFace".to_string(),
                id: "huggingface".to_string(),
                models: hf_models,
            },
            ProviderModels {
                name: "Ollama (local)".to_string(),
                id: "ollama".to_string(),
                models: ollama_models,
            },
            ProviderModels {
                name: "OpenAI-compatible".to_string(),
                id: "openai-compatible".to_string(),
                models: openai_models,
            },
        ],
        current_provider,
        current_model,
    })
}

async fn fetch_ollama_models() -> Vec<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default();

    match client
        .get("http://localhost:11434/api/tags")
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                if let Some(models) = body["models"].as_array() {
                    let mut names: Vec<String> = models
                        .iter()
                        .filter_map(|m| m["name"].as_str().map(|s| s.to_string()))
                        .collect();
                    if !names.is_empty() {
                        names.sort();
                        return names;
                    }
                }
            }
            curated_ollama_models()
        }
        _ => curated_ollama_models(),
    }
}

fn curated_ollama_models() -> Vec<String> {
    vec![
        "qwen2.5-coder:32b".to_string(),
        "qwen2.5-coder:14b".to_string(),
        "qwen2.5-coder:7b".to_string(),
        "codellama:13b".to_string(),
        "codellama:7b".to_string(),
        "deepseek-coder-v2:16b".to_string(),
        "deepseek-coder:6.7b".to_string(),
        "llama3.3:70b".to_string(),
        "mistral:7b".to_string(),
    ]
}

/// Fetch the live model list from HuggingFace's /v1/models endpoint.
/// Falls back to a small curated list if the request fails.
async fn fetch_hf_models() -> Vec<String> {
    let token = std::env::var("HF_TOKEN").unwrap_or_default();
    if token.is_empty() {
        return curated_hf_models();
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    match client
        .get("https://router.huggingface.co/v1/models")
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                if let Some(models) = body["data"].as_array() {
                    let mut names: Vec<String> = models
                        .iter()
                        .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                        .collect();
                    if !names.is_empty() {
                        // Sort: put coding-oriented models first, then alphabetical
                        names.sort_by(|a, b| {
                            let a_code = a.to_lowercase().contains("coder")
                                || a.to_lowercase().contains("code");
                            let b_code = b.to_lowercase().contains("coder")
                                || b.to_lowercase().contains("code");
                            match (a_code, b_code) {
                                (true, false) => std::cmp::Ordering::Less,
                                (false, true) => std::cmp::Ordering::Greater,
                                _ => a.cmp(b),
                            }
                        });
                        return names;
                    }
                }
            }
            curated_hf_models()
        }
        _ => curated_hf_models(),
    }
}

/// Fallback HF model list when the API is unreachable or token is missing.
fn curated_hf_models() -> Vec<String> {
    vec![
        "Qwen/Qwen2.5-Coder-32B-Instruct".to_string(),
        "Qwen/Qwen2.5-Coder-7B-Instruct".to_string(),
        "meta-llama/Llama-3.3-70B-Instruct".to_string(),
        "meta-llama/Llama-3.1-8B-Instruct".to_string(),
        "deepseek-ai/DeepSeek-R1".to_string(),
        "Qwen/Qwen3-32B".to_string(),
    ]
}

// ══════════════════════════════════════════════════════════════════════
//  Runtime Settings
// ══════════════════════════════════════════════════════════════════════

/// GET /api/settings — return current runtime settings
pub async fn get_settings(
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    let settings = state.runtime_settings.read().await;
    Json(settings.clone())
}

/// POST /api/settings — update runtime settings
pub async fn update_settings(
    State(state): State<Arc<DashboardState>>,
    Json(new_settings): Json<RuntimeSettings>,
) -> impl IntoResponse {
    let mut settings = state.runtime_settings.write().await;
    *settings = new_settings;
    Json(serde_json::json!({ "status": "ok" }))
}

// ══════════════════════════════════════════════════════════════════════
//  View types for templates
// ══════════════════════════════════════════════════════════════════════

#[derive(Clone, Debug, Serialize)]
pub struct ChatMessageView {
    pub role: String,
    pub content: String,
    pub is_code: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ContainerInfo {
    pub id: String,
    pub image: String,
    pub status: String,
    pub names: String,
}

// ══════════════════════════════════════════════════════════════════════
//  Code Viewing & Containers
// ══════════════════════════════════════════════════════════════════════

/// GET /code/:filename — view a generated script
pub async fn view_code(
    State(state): State<Arc<DashboardState>>,
    axum::extract::Path(filename): axum::extract::Path<String>,
) -> impl IntoResponse {
    let path = std::path::Path::new(&state.config.generated_dir).join(&filename);
    match std::fs::read_to_string(&path) {
        Ok(code) => Html(templates::render_code_block(&code)),
        Err(_) => Html(format!(
            "<p class=\"text-red-400\">File not found: {}</p>",
            html_escape(&filename)
        )),
    }
}

/// GET /api/containers — list running Docker containers as JSON
pub async fn get_containers() -> impl IntoResponse {
    let containers = list_docker_containers().await;
    Json(containers)
}

/// GET /api/containers/html — HTML partial for HTMX
pub async fn get_containers_html() -> impl IntoResponse {
    let containers = list_docker_containers().await;
    Html(templates::render_containers(&containers))
}

// ══════════════════════════════════════════════════════════════════════
//  Helpers
// ══════════════════════════════════════════════════════════════════════

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
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "py"))
        .map(|e| {
            let filename = e.file_name().to_string_lossy().to_string();
            let path = e.path().display().to_string();
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
        .args([
            "ps",
            "--filter",
            "ancestor=python-sandbox",
            "--format",
            "{{.ID}}\t{{.Image}}\t{{.Status}}\t{{.Names}}",
        ])
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

fn now_hms() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}
