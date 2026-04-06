use crate::api::{self, Message, Provider};
use crate::config::AppConfig;
use crate::dashboard::state::{DashboardState, ExecutionEvent};
use crate::logger::{Logger, SessionMetrics};
use crate::python_exec::{CodeExecutor, ExecutionMode, LintSeverity, SecuritySeverity};
use crate::rag::{self, RagStore};
use crate::utils::{extract_python_code, find_char_boundary};
use colored::*;
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::hint::Hinter;
use rustyline::{CompletionType, Config, Context, Editor, Helper, Highlighter, Validator};
use std::fs;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Available slash commands for tab-completion.
const COMMANDS: &[&str] = &[
    "/help",
    "/quit",
    "/exit",
    "/clear",
    "/refine",
    "/save",
    "/history",
    "/stats",
    "/list",
    "/run",
    "/provider",
    "/lint",
    "/security",
    "/dashboard",
    "/context",
    "/explain",
    "/project",
    "/session",
];

/// Rustyline helper providing slash-command tab-completion and inline hints.
#[derive(Helper, Validator, Highlighter)]
struct CommandCompleter;

impl Hinter for CommandCompleter {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> Option<String> {
        // Only hint when cursor is at end and line starts with '/'
        if pos != line.len() || !line.starts_with('/') || line.contains(' ') {
            return None;
        }

        // Find the first command that matches and return the remaining suffix as hint
        COMMANDS
            .iter()
            .find(|cmd| cmd.starts_with(line) && **cmd != line)
            .map(|cmd| cmd[line.len()..].to_string())
    }
}

impl Completer for CommandCompleter {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        // Only complete when the cursor is at the first word and it starts with '/'
        let prefix = &line[..pos];
        if !prefix.starts_with('/') || prefix.contains(' ') {
            return Ok((0, vec![]));
        }

        let matches: Vec<Pair> = COMMANDS
            .iter()
            .filter(|cmd| cmd.starts_with(prefix))
            .map(|cmd| Pair {
                display: cmd.to_string(),
                replacement: cmd.to_string(),
            })
            .collect();

        Ok((0, matches))
    }
}

// Public function called from main.rs to display the welcome banner
pub fn print_banner() {
    // Clear screen first
    print!("\x1B[2J\x1B[1;1H");

    let art = r#"
   ██████╗ ██╗   ██╗████████╗██╗  ██╗ ██████╗ ███╗   ██╗
   ██╔══██╗╚██╗ ██╔╝╚══██╔══╝██║  ██║██╔═══██╗████╗  ██║
   ██████╔╝ ╚████╔╝    ██║   ███████║██║   ██║██╔██╗ ██║
   ██╔═══╝   ╚██╔╝     ██║   ██╔══██║██║   ██║██║╚██╗██║
   ██║        ██║      ██║   ██║  ██║╚██████╔╝██║ ╚████║
   ╚═╝        ╚═╝      ╚═╝   ╚═╝  ╚═╝ ╚═════╝ ╚═╝  ╚═══╝
    "#;
    println!("{}", art.bright_cyan().bold());
    println!(
        "    {}",
        "MAKER BOT v0.4.0 — AI Code Generator".bright_white()
    );
    println!();
    println!(
        "    {} Type {} for command list",
        "ℹ".cyan(),
        "/help".bold().white()
    );
    println!("    {} Type {} to quit", "ℹ".cyan(), "/quit".bold().white());
    println!();
}

// Utility function to ask the user a question and return their answer
pub fn ask_user(question: &str) -> String {
    print!("{question}");
    if io::stdout().flush().is_err() {
        return String::new();
    }

    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return String::new();
    }
    input.trim().to_string()
}

// Utility function that asks a yes/no question using ask_user
pub fn confirm(question: &str) -> bool {
    let ans = ask_user(&format!("{question} (y/n) : "));
    ans.to_lowercase().starts_with('y')
}

// Display function for generated Python code
pub fn display_code(code: &str) {
    let border = "────────────────────────────────────────────────────────".bright_black();
    println!("\n{}", border);
    println!("  {}", "Generated Python Code".bright_cyan().bold());
    println!("{}", border);

    // Simple syntax highlighting for Python
    for (i, line) in code.lines().enumerate() {
        let line_num = format!("{:3} │", i + 1).bright_black();
        let trimmed = line.trim_start();
        let highlighted = if trimmed.starts_with('#') {
            line.bright_green() // Comments green
        } else if trimmed.starts_with("def ") || trimmed.starts_with("class ") {
            line.bright_yellow()
        } else if trimmed.starts_with("import ") || trimmed.starts_with("from ") {
            line.bright_magenta()
        } else if trimmed.contains("print(") {
            line.cyan()
        } else {
            line.white()
        };
        println!("{} {}", line_num, highlighted);
    }
    println!("{}", border);
    println!();
}

/// Trim conversation history to at most `max` messages, dropping the oldest
/// user/assistant pairs first.
pub fn trim_history(history: &mut Vec<Message>, max: usize) {
    while history.len() > max {
        // Remove in pairs (user + assistant) from the front
        if history.len() >= 2 {
            history.drain(..2);
        } else {
            history.remove(0);
        }
    }
}

/// Start a spinner animation in a background thread.
/// Returns an `Arc<AtomicBool>` — set it to `false` to stop the spinner.
fn start_spinner(message: &str) -> Arc<AtomicBool> {
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();
    let msg = message.to_string();

    std::thread::spawn(move || {
        let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let mut i = 0;
        while running_clone.load(Ordering::Relaxed) {
            print!(
                "\r{} {} ",
                frames[i % frames.len()].to_string().cyan(),
                msg.dimmed()
            );
            let _ = io::stdout().flush();
            std::thread::sleep(std::time::Duration::from_millis(80));
            i += 1;
        }
        // Clear the spinner line
        print!("\r{}\r", " ".repeat(msg.len() + 4));
        let _ = io::stdout().flush();
    });

    running
}

/// Stop a running spinner.
fn stop_spinner(handle: &Arc<AtomicBool>) {
    handle.store(false, Ordering::Relaxed);
    // Give the spinner thread time to clear the line
    std::thread::sleep(std::time::Duration::from_millis(100));
}

/// Shared initialization context for the REPL, used by both standalone
/// and dashboard-enabled entry points.
struct ReplContext {
    executor: CodeExecutor,
    logger: Logger,
    metrics: SessionMetrics,
    linter_available: bool,
    security_scanner_available: bool,
    /// Resolved Docker availability (may differ from config if Docker is unavailable).
    use_docker: bool,
}

/// Validate provider, check tool availability, create executor/logger.
/// Returns `None` if provider configuration is invalid (errors are printed).
fn init_repl_context(config: &AppConfig) -> Option<ReplContext> {
    // Validate and display the configured provider
    let provider = match Provider::from_config(&config.provider) {
        Ok(p) => p,
        Err(e) => {
            println!("{} {}", "✗ Invalid provider configuration:".red().bold(), e);
            return None;
        }
    };
    match provider.resolve_api_url(&config.api_url) {
        Ok(url) => println!(
            "{} {} → {}",
            "✔ Provider:".green(),
            provider.display_name().bright_white(),
            url.dimmed()
        ),
        Err(e) => {
            println!("{} {}", "✖ Provider configuration error:".red().bold(), e);
            return None;
        }
    }

    if config.use_venv {
        println!(
            "{} {}",
            "✔".green(),
            "Virtual environment isolation enabled.".white()
        );
    }

    // Check linter availability
    let linter_available = if config.use_linting {
        if CodeExecutor::check_linter_available() {
            println!("{} {}", "✔".green(), "Linting enabled (ruff).".white());
            true
        } else {
            println!(
                "{} Linting enabled but ruff not found. Install with: pip install ruff",
                "⚠".yellow()
            );
            println!("  {} Linting will be skipped.", "ℹ".blue());
            false
        }
    } else {
        false
    };

    // Check security scanner (bandit) availability
    let security_scanner_available = if config.use_security_check {
        if CodeExecutor::check_security_scanner_available() {
            println!(
                "{} {}",
                "✔".green(),
                "Security scanning enabled (bandit).".white()
            );
            true
        } else {
            println!("{} Security scanning enabled but bandit not found. Install with: pip install bandit", "⚠".yellow());
            println!("  {} Security scanning will be skipped.", "ℹ".blue());
            false
        }
    } else {
        false
    };

    // If Docker mode is enabled, verify Docker is available; fall back to host execution if not
    let use_docker = if config.use_docker {
        print!("{} Checking Docker availability...", "⟳".dimmed());
        std::io::Write::flush(&mut std::io::stdout()).ok();
        match CodeExecutor::check_docker_available() {
            Ok(()) => {
                print!("\r\x1b[2K");
                println!("{} {}", "✔".green(), "Docker sandbox mode enabled.".white());
                true
            }
            Err(e) => {
                print!("\r\x1b[2K");
                println!("{} {}", "✖ Docker sandbox not available:".red().bold(), e);
                println!("  {} Falling back to host execution.", "⚠".yellow());
                println!(
                    "  {} To enable Docker, run: docker build -t python-sandbox .",
                    "ℹ".blue()
                );
                false
            }
        }
    } else {
        false
    };

    let executor = CodeExecutor::new(
        &config.generated_dir,
        use_docker,
        config.use_venv,
        &config.python_executable,
    )
    .expect("Failed to create generated scripts directory");
    let logger = Logger::new(&config.log_dir).expect("Failed to create logger");
    let metrics = SessionMetrics::new();

    Some(ReplContext {
        executor,
        logger,
        metrics,
        linter_available,
        security_scanner_available,
        use_docker,
    })
}

// Interactive REPL entry point
pub async fn start_repl(config: &AppConfig) {
    print_banner();

    let config_clone = config.clone();
    let ctx = match tokio::task::spawn_blocking(move || init_repl_context(&config_clone))
        .await
        .expect("init_repl_context task panicked")
    {
        Some(c) => c,
        None => return,
    };

    start_repl_loop(
        config,
        ctx.executor,
        ctx.logger,
        ctx.metrics,
        ctx.linter_available,
        ctx.security_scanner_available,
        None,
    )
    .await;
}

/// Start the REPL with the web dashboard running in the background.
///
/// Creates shared state, spawns the Axum dashboard server, then runs
/// the same REPL loop with dashboard event broadcasting enabled.
pub async fn start_repl_with_dashboard(config: &AppConfig) {
    print_banner();

    let config_clone = config.clone();
    let ctx = match tokio::task::spawn_blocking(move || init_repl_context(&config_clone))
        .await
        .expect("init_repl_context task panicked")
    {
        Some(c) => c,
        None => return,
    };

    // Create a second executor for the dashboard's REST API
    let dashboard_executor = CodeExecutor::new(
        &config.generated_dir,
        ctx.use_docker,
        config.use_venv,
        &config.python_executable,
    )
    .expect("Failed to create generated scripts directory");

    // Create shared dashboard state and spawn the web server
    let state = DashboardState::new(config.clone(), dashboard_executor);
    let dashboard_port = config.dashboard_port;

    let server_state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = crate::dashboard::start_dashboard(server_state, dashboard_port).await {
            eprintln!("{} {}", "✗ Dashboard server error:".red(), e);
        }
    });

    println!(
        "{} {}",
        "✓ Dashboard running at:".green(),
        format!("http://localhost:{}", dashboard_port)
            .bright_white()
            .underline()
    );

    start_repl_loop(
        config,
        ctx.executor,
        ctx.logger,
        ctx.metrics,
        ctx.linter_available,
        ctx.security_scanner_available,
        Some(state),
    )
    .await;
}

async fn start_repl_loop(
    config: &AppConfig,
    executor: CodeExecutor,
    logger: Logger,
    mut metrics: SessionMetrics,
    linter_available: bool,
    security_scanner_available: bool,
    dashboard: Option<Arc<DashboardState>>,
) {
    // Set up rustyline editor with tab-completion
    let rl_config = Config::builder()
        .auto_add_history(true)
        .completion_type(CompletionType::List)
        .completion_prompt_limit(100)
        .build();
    let mut rl = Editor::with_config(rl_config).expect("Failed to create line editor");
    rl.set_helper(Some(CommandCompleter));

    // Conversation history for multi-turn refinement
    let mut conversation_history: Vec<Message> = Vec::new();
    let mut last_generated_code = String::new();
    let mut rag_store = RagStore::new();

    // Track last synced metrics for delta-based dashboard updates
    let mut last_synced_metrics = SessionMetrics::new();

    loop {
        // Two-line prompt for better visibility
        let prompt = format!(
            "\n{} {}\n{} ",
            "╭──".bright_black(),
            "🤖".yellow(),
            "╰── ➤".bright_magenta()
        );
        let readline = rl.readline(&prompt);
        let prompt = match readline {
            Ok(line) => line.trim().to_string(),
            Err(ReadlineError::Interrupted | ReadlineError::Eof) => {
                println!("Goodbye!");
                break;
            }
            Err(e) => {
                println!("{} {}", "✗ Input error:".red(), e);
                continue;
            }
        };

        if prompt.is_empty() {
            continue;
        }

        if prompt == "/quit" || prompt == "/exit" {
            println!("Goodbye!");
            break;
        }

        if prompt == "/help" {
            let bar = "│".bright_black();
            println!(
                "\n{}",
                "  ╭── Available Commands ──────────────────────".bright_black()
            );
            println!(
                "  {bar} {}    Exit the program",
                "/quit, /exit".green().bold()
            );
            println!(
                "  {bar} {}         Show this help output",
                "/help".green().bold()
            );
            println!(
                "  {bar} {}        Clear conversation history",
                "/clear".green().bold()
            );
            println!(
                "  {bar} {}       Refine the last generated code",
                "/refine".green().bold()
            );
            println!(
                "  {bar} {} <file> Save last code to a file",
                "/save".green().bold()
            );
            println!(
                "  {bar} {}      Show conversation history",
                "/history".green().bold()
            );
            println!(
                "  {bar} {}        Show session statistics",
                "/stats".green().bold()
            );
            println!(
                "  {bar} {}         List all previously generated scripts",
                "/list".green().bold()
            );
            println!(
                "  {bar} {} <file>  Execute a previously generated script",
                "/run".green().bold()
            );
            println!(
                "  {bar} {}     Show current LLM provider info",
                "/provider".green().bold()
            );
            println!(
                "  {bar} {}         Lint the last generated code (ruff)",
                "/lint".green().bold()
            );
            println!(
                "  {bar} {}     Run security scan (bandit)",
                "/security".green().bold()
            );
            println!(
                "  {bar} {}    Show dashboard URL",
                "/dashboard".green().bold()
            );
            println!(
                "  {bar} {} <file> Load file as RAG context (txt/md/csv)",
                "/context".green().bold()
            );
            println!(
                "  {bar} {}      Explain the last generated code",
                "/explain".green().bold()
            );
            println!(
                "  {bar} {} <desc> Generate a multi-file project",
                "/project".green().bold()
            );
            println!(
                "  {bar} {} save/load/list  Manage sessions",
                "/session".green().bold()
            );
            println!(
                "{}",
                "  ╰────────────────────────────────────────────".bright_black()
            );
            println!();
            continue;
        }

        if prompt == "/dashboard" {
            if let Some(ref ds) = dashboard {
                println!(
                    "{} {}",
                    "Dashboard running at:".bright_cyan(),
                    format!("http://localhost:{}", ds.config.dashboard_port)
                        .bright_white()
                        .underline()
                );
            } else {
                println!(
                    "{}",
                    "Dashboard is not enabled. Set enable_dashboard = true in pymakebot.toml"
                        .yellow()
                );
            }
            continue;
        }

        if prompt == "/stats" {
            metrics.display();
            continue;
        }

        if prompt == "/provider" {
            if let Ok(p) = Provider::from_config(&config.provider) {
                println!("\n{}", "LLM Provider Info:".bright_cyan().bold());
                println!(
                    "  {} {}",
                    "Provider:".dimmed(),
                    p.display_name().bright_white()
                );
                println!("  {}    {}", "Model:".dimmed(), config.model.bright_white());
                if let Ok(url) = p.resolve_api_url(&config.api_url) {
                    println!("  {}  {}", "API URL:".dimmed(), url.bright_white());
                }
                println!();
            }
            continue;
        }

        // /lint command — run ruff on the last generated code
        if prompt == "/lint" {
            if last_generated_code.is_empty() {
                println!("{}", "No code to lint. Generate some code first!".yellow());
                continue;
            }
            if !linter_available {
                println!(
                    "{}",
                    "Linter (ruff) is not available. Install with: pip install ruff".yellow()
                );
                continue;
            }
            // Write to a temp file for linting
            match executor.write_script(&last_generated_code) {
                Ok(path) => match executor.lint_check(&path) {
                    Ok(lint_result) => display_lint_results(&lint_result),
                    Err(e) => println!("{} {}", "✗ Lint error:".red(), e),
                },
                Err(e) => println!("{} {}", "✗ Failed to write script for linting:".red(), e),
            }
            continue;
        }

        // /security command — run bandit on the last generated code
        if prompt == "/security" {
            if last_generated_code.is_empty() {
                println!("{}", "No code to scan. Generate some code first!".yellow());
                continue;
            }
            if !security_scanner_available {
                println!(
                    "{}",
                    "Security scanner (bandit) is not available. Install with: pip install bandit"
                        .yellow()
                );
                continue;
            }
            match executor.write_script(&last_generated_code) {
                Ok(path) => match executor.security_check(&path) {
                    Ok(sec_result) => display_security_results(&sec_result),
                    Err(e) => println!("{} {}", "✗ Security scan error:".red(), e),
                },
                Err(e) => println!("{} {}", "✗ Failed to write script for scanning:".red(), e),
            }
            continue;
        }

        if prompt == "/clear" {
            conversation_history.clear();
            last_generated_code.clear();
            println!("{}", "✓ Conversation history cleared.".green());
            continue;
        }

        if prompt == "/history" {
            if conversation_history.is_empty() {
                println!("{}", "No conversation history yet.".yellow());
            } else {
                println!(
                    "\n{}",
                    "  ╭── Conversation History ────────────────────".bright_cyan()
                );
                for (i, msg) in conversation_history.iter().enumerate() {
                    let role_color = if msg.role == "user" {
                        msg.role.bright_blue()
                    } else {
                        msg.role.bright_green()
                    };
                    let preview = if msg.content.len() > 80 {
                        let end = find_char_boundary(&msg.content, 80);
                        format!("{}...", &msg.content[..end]).replace('\n', " ")
                    } else {
                        msg.content.replace('\n', " ")
                    };
                    println!(
                        "  {} {}. [{}] {}",
                        "│".bright_cyan(),
                        i + 1,
                        role_color,
                        preview.dimmed()
                    );
                }
                println!(
                    "{}",
                    "  ╰────────────────────────────────────────────".bright_cyan()
                );
                println!();
            }
            continue;
        }

        if prompt.starts_with("/save") {
            if last_generated_code.is_empty() {
                println!("{}", "No code to save. Generate some code first!".yellow());
                continue;
            }

            let parts: Vec<&str> = prompt.split_whitespace().collect();
            let filename = if parts.len() > 1 {
                parts[1].to_string()
            } else {
                ask_user("Enter filename (e.g., script.py): ")
            };

            if filename.is_empty() {
                println!("{}", "Save cancelled.".yellow());
                continue;
            }

            match fs::write(&filename, &last_generated_code) {
                Ok(_) => println!("{} {}", "✓ Code saved to:".green(), filename.bright_white()),
                Err(e) => println!("{} {}", "✗ Failed to save file:".red(), e),
            }
            continue;
        }

        if prompt == "/list" {
            match fs::read_dir(&config.generated_dir) {
                Ok(entries) => {
                    let mut scripts: Vec<_> = entries
                        .filter_map(|e| e.ok())
                        .filter(|e| e.path().extension().is_some_and(|ext| ext == "py"))
                        .collect();

                    if scripts.is_empty() {
                        println!("{}", "No generated scripts found.".yellow());
                    } else {
                        scripts.sort_by_key(|e| e.file_name());
                        println!(
                            "\n{}",
                            "  ╭── Generated Scripts ───────────────────────".bright_cyan()
                        );
                        for (i, entry) in scripts.iter().enumerate() {
                            println!(
                                "  {} {}. {}",
                                "│".bright_cyan(),
                                i + 1,
                                entry.file_name().to_string_lossy().bright_white()
                            );
                        }
                        println!(
                            "{}",
                            "  ╰────────────────────────────────────────────".bright_cyan()
                        );
                        println!();
                    }
                }
                Err(e) => println!("{} {}", "✖ Failed to list scripts:".red(), e),
            }
            continue;
        }

        if prompt.starts_with("/run") {
            let parts: Vec<&str> = prompt.split_whitespace().collect();
            let filename = if parts.len() > 1 {
                parts[1].to_string()
            } else {
                ask_user("Enter script filename (e.g., script_20251209_152023.py): ")
            };

            if filename.is_empty() {
                println!("{}", "Run cancelled.".yellow());
                continue;
            }

            let script_path = if filename.starts_with(&format!("{}/", config.generated_dir)) {
                filename
            } else {
                format!("{}/{}", config.generated_dir, filename)
            };

            match fs::read_to_string(&script_path) {
                Ok(code) => {
                    println!("\n{}", format!("Running: {}", script_path).bright_cyan());

                    // Create a venv for this execution (host mode only)
                    let venv = executor.create_venv().unwrap_or_else(|e| {
                        println!("{} {}", "⚠️  Failed to create venv:".yellow(), e);
                        println!("{}", "Proceeding without virtual environment...".dimmed());
                        None
                    });

                    // Check for dependencies
                    let deps = executor.detect_dependencies(&code);
                    if !deps.is_empty() {
                        println!(
                            "\n{} {}",
                            "⚠️  Detected non-standard dependencies:".yellow(),
                            deps.join(", ").bright_yellow()
                        );
                        if config.auto_install_deps || confirm("Install these dependencies?") {
                            if let Err(e) = executor.install_packages(&deps, venv.as_deref()) {
                                println!(
                                    "{} {}",
                                    "⚠️  Failed to install dependencies:".yellow(),
                                    e
                                );
                                println!("{}", "Proceeding anyway...".dimmed());
                            }
                        }
                    }

                    // Detect if interactive mode is needed
                    let mode = if executor.needs_interactive_mode(&code) {
                        println!(
                            "{}",
                            "🎮 Interactive mode detected (pygame/input/GUI)"
                                .bright_magenta()
                                .bold()
                        );
                        println!(
                            "{}",
                            "   Running with inherited stdio for user interaction...".dimmed()
                        );
                        ExecutionMode::Interactive
                    } else {
                        ExecutionMode::Captured
                    };

                    match executor.run_existing_script(
                        &script_path,
                        mode,
                        config.execution_timeout_secs,
                        venv.as_deref(),
                        &deps,
                    ) {
                        Ok(result) => {
                            let success = result.is_success();
                            if success {
                                metrics.successful_executions += 1;
                            } else {
                                metrics.failed_executions += 1;
                            }

                            let _ = logger.log_execution(success, &result.stdout);

                            println!(
                                "\n{}",
                                "━━━━━━━━━━━ Execution Result ━━━━━━━━━━━"
                                    .bright_blue()
                                    .bold()
                            );
                            if !result.stdout.is_empty() {
                                println!("\n{}:", "STDOUT".green().bold());
                                println!("{}", result.stdout);
                            }
                            if !result.stderr.is_empty() {
                                println!("\n{}:", "STDERR".red().bold());
                                println!("{}", result.stderr);
                            }
                            println!(
                                "{}",
                                "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_blue()
                            );
                        }
                        Err(e) => {
                            metrics.failed_executions += 1;
                            let _ = logger.log_error(&format!("Execution error: {}", e));
                            println!("{} {}", "✗ Execution error:".red(), e);
                        }
                    }

                    // Clean up the venv
                    if let Some(ref venv_path) = venv {
                        executor.cleanup_venv(venv_path);
                    }
                }
                Err(e) => println!("{} {}", "✗ Failed to read script:".red(), e),
            }
            continue;
        }

        if prompt.starts_with("/context") {
            if !config.enable_rag {
                println!(
                    "{}",
                    "RAG is disabled. Set enable_rag = true in pymakebot.toml".yellow()
                );
                continue;
            }

            let parts: Vec<&str> = prompt.splitn(2, ' ').collect();
            let file_path = if parts.len() > 1 {
                parts[1].to_string()
            } else {
                ask_user("Enter file path (txt/md/csv): ")
            };

            if file_path.is_empty() {
                println!("{}", "Context load cancelled.".yellow());
                continue;
            }

            let spinner = start_spinner("Loading and embedding file...");
            match rag_store.load_file(&file_path, config).await {
                Ok(count) => {
                    stop_spinner(&spinner);
                    println!(
                        "{} Loaded {} chunks from {}",
                        "✓".green(),
                        count.to_string().bright_white(),
                        file_path.bright_cyan()
                    );
                }
                Err(e) => {
                    stop_spinner(&spinner);
                    println!("{} {}", "✗ Failed to load context:".red(), e);
                }
            }
            continue;
        }

        // /explain command — ask the LLM to explain the last generated code
        if prompt == "/explain" {
            if last_generated_code.is_empty() {
                println!(
                    "{}",
                    "No code to explain. Generate some code first!".yellow()
                );
                continue;
            }
            let spinner = start_spinner("Generating explanation...");
            match api::explain_code(&last_generated_code, config).await {
                Ok(explanation) => {
                    stop_spinner(&spinner);
                    println!(
                        "\n{}",
                        "━━━━━━━━━━━ Code Explanation ━━━━━━━━━━━"
                            .bright_cyan()
                            .bold()
                    );
                    println!("{}", explanation);
                    println!(
                        "{}",
                        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_cyan()
                    );
                }
                Err(e) => {
                    stop_spinner(&spinner);
                    println!("{} {}", "✗ Explanation error:".red(), e);
                }
            }
            continue;
        }

        // /project command — generate a multi-file project structure
        if prompt.starts_with("/project") {
            let parts: Vec<&str> = prompt.splitn(2, ' ').collect();
            let project_prompt = if parts.len() > 1 {
                parts[1].to_string()
            } else {
                ask_user("Describe the project you want to generate: ")
            };

            if project_prompt.is_empty() {
                println!("{}", "Project generation cancelled.".yellow());
                continue;
            }

            let spinner = start_spinner("Generating project structure...");
            match api::generate_project(&project_prompt, config).await {
                Ok(raw_response) => {
                    stop_spinner(&spinner);
                    match crate::utils::parse_project_blueprint(&raw_response) {
                        Ok(blueprint) => {
                            match crate::python_exec::scaffold_project(
                                &blueprint,
                                &config.generated_dir,
                            ) {
                                Ok(project_dir) => {
                                    println!(
                                        "\n{} {}",
                                        "✓ Project created:".green(),
                                        blueprint.project_name.bright_white().bold()
                                    );
                                    println!(
                                        "  {} {}",
                                        "Description:".dimmed(),
                                        blueprint.description
                                    );
                                    println!("  {} {:?}", "Location:".dimmed(), project_dir);
                                    println!("\n{}", "  Files generated:".bright_cyan());
                                    for file in &blueprint.files {
                                        println!("    {} {}", "•".bright_cyan(), file.path.white());
                                    }
                                    println!();
                                }
                                Err(e) => {
                                    println!("{} {}", "✗ Failed to scaffold project:".red(), e)
                                }
                            }
                        }
                        Err(e) => {
                            println!("{} {}", "✗ Failed to parse project structure:".red(), e);
                            println!("{}", "The LLM may not have returned valid JSON. Try rephrasing your request.".dimmed());
                        }
                    }
                }
                Err(e) => {
                    stop_spinner(&spinner);
                    println!("{} {}", "✗ API error:".red(), e);
                }
            }
            metrics.total_requests += 1;
            continue;
        }

        // /session command — save, load, or list conversation sessions
        if prompt.starts_with("/session") {
            let parts: Vec<&str> = prompt.splitn(3, ' ').collect();
            let subcommand = parts.get(1).copied().unwrap_or("");

            match subcommand {
                "save" => {
                    let name = if parts.len() > 2 {
                        parts[2].to_string()
                    } else {
                        ask_user("Enter session name: ")
                    };
                    if name.is_empty() {
                        println!("{}", "Save cancelled.".yellow());
                        continue;
                    }
                    let sessions_dir = std::path::Path::new(&config.sessions_dir);
                    if let Err(e) = fs::create_dir_all(sessions_dir) {
                        println!("{} {}", "✗ Failed to create sessions directory:".red(), e);
                        continue;
                    }
                    let session_data = serde_json::json!({
                        "name": name,
                        "timestamp": chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                        "provider": config.provider,
                        "model": config.model,
                        "messages": conversation_history,
                        "last_generated_code": last_generated_code,
                    });
                    let filename = format!("{}.json", name.replace(' ', "_"));
                    let filepath = sessions_dir.join(&filename);
                    match fs::write(
                        &filepath,
                        serde_json::to_string_pretty(&session_data).unwrap_or_default(),
                    ) {
                        Ok(_) => println!(
                            "{} {}",
                            "✓ Session saved:".green(),
                            filepath.display().to_string().bright_white()
                        ),
                        Err(e) => println!("{} {}", "✗ Failed to save session:".red(), e),
                    }
                }
                "load" => {
                    let name = if parts.len() > 2 {
                        parts[2].to_string()
                    } else {
                        ask_user("Enter session name: ")
                    };
                    if name.is_empty() {
                        println!("{}", "Load cancelled.".yellow());
                        continue;
                    }
                    let filename = format!("{}.json", name.replace(' ', "_"));
                    let filepath = std::path::Path::new(&config.sessions_dir).join(&filename);
                    match fs::read_to_string(&filepath) {
                        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
                            Ok(data) => {
                                if let Some(messages) = data.get("messages") {
                                    if let Ok(msgs) =
                                        serde_json::from_value::<Vec<Message>>(messages.clone())
                                    {
                                        conversation_history = msgs;
                                    }
                                }
                                if let Some(code) =
                                    data.get("last_generated_code").and_then(|v| v.as_str())
                                {
                                    last_generated_code = code.to_string();
                                }
                                println!(
                                    "{} Loaded session '{}' ({} messages)",
                                    "✓".green(),
                                    name.bright_white(),
                                    conversation_history.len()
                                );
                            }
                            Err(e) => println!("{} {}", "✗ Failed to parse session file:".red(), e),
                        },
                        Err(e) => println!("{} {}", "✗ Failed to read session file:".red(), e),
                    }
                }
                "list" => {
                    let sessions_dir = std::path::Path::new(&config.sessions_dir);
                    if !sessions_dir.exists() {
                        println!("{}", "No saved sessions found.".yellow());
                        continue;
                    }
                    match fs::read_dir(sessions_dir) {
                        Ok(entries) => {
                            let mut sessions: Vec<_> = entries
                                .filter_map(|e| e.ok())
                                .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
                                .collect();
                            if sessions.is_empty() {
                                println!("{}", "No saved sessions found.".yellow());
                            } else {
                                sessions.sort_by_key(|e| e.file_name());
                                println!(
                                    "\n{}",
                                    "  ╭── Saved Sessions ──────────────────────────".bright_cyan()
                                );
                                for (i, entry) in sessions.iter().enumerate() {
                                    let name = entry
                                        .path()
                                        .file_stem()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .to_string();
                                    println!(
                                        "  {} {}. {}",
                                        "│".bright_cyan(),
                                        i + 1,
                                        name.bright_white()
                                    );
                                }
                                println!(
                                    "{}",
                                    "  ╰────────────────────────────────────────────".bright_cyan()
                                );
                                println!();
                            }
                        }
                        Err(e) => println!("{} {}", "✗ Failed to list sessions:".red(), e),
                    }
                }
                _ => {
                    println!(
                        "{}",
                        "Usage: /session save <name> | /session load <name> | /session list"
                            .yellow()
                    );
                }
            }
            continue;
        }

        if prompt == "/refine" {
            if last_generated_code.is_empty() {
                println!(
                    "{}",
                    "No code to refine. Generate some code first!".yellow()
                );
                continue;
            }
            print!("{}", "What would you like to change or add? ".cyan());
            io::stdout().flush().unwrap();
            let mut refinement = String::new();
            io::stdin().read_line(&mut refinement).unwrap();
            let refinement = refinement.trim();

            if refinement.is_empty() {
                continue;
            }

            // Add refinement request to history (with optional RAG augmentation)
            let refine_content = format!("Please refine the previous code: {}", refinement);
            let final_refine = if config.enable_rag && !rag_store.is_empty() {
                match rag_store.retrieve(refinement, 3, config).await {
                    Ok(chunks) if !chunks.is_empty() => {
                        rag::build_rag_prompt(&chunks, &refine_content)
                    }
                    _ => refine_content,
                }
            } else {
                refine_content
            };
            conversation_history.push(Message {
                role: "user".to_string(),
                content: final_refine,
            });
        } else {
            // Regular prompt - add to history (with optional RAG augmentation)
            let final_prompt = if config.enable_rag && !rag_store.is_empty() {
                match rag_store.retrieve(&prompt, 3, config).await {
                    Ok(chunks) if !chunks.is_empty() => rag::build_rag_prompt(&chunks, &prompt),
                    _ => prompt.clone(),
                }
            } else {
                prompt.clone()
            };
            conversation_history.push(Message {
                role: "user".to_string(),
                content: final_prompt,
            });
        }

        // Log the request
        let _ = logger.log_api_request(&conversation_history.last().unwrap().content);
        metrics.total_requests += 1;

        // Call the LLM — use streaming if enabled
        let api_result = if config.use_streaming {
            // Streaming mode: print tokens as they arrive
            print!("\n{} ", "⟩".bright_cyan());
            let _ = io::stdout().flush();
            match api::generate_code_stream(&conversation_history, config).await {
                Ok((chunks, full_content)) => {
                    // Print each chunk as it arrives (simulate streaming effect)
                    for chunk in &chunks {
                        print!("{}", chunk.dimmed());
                        let _ = io::stdout().flush();
                    }
                    println!();
                    Ok(full_content)
                }
                Err(e) => {
                    // Fall back to non-streaming on error
                    println!();
                    let spinner = start_spinner("Falling back to non-streaming...");
                    let result =
                        api::generate_code_with_history(&conversation_history, config).await;
                    stop_spinner(&spinner);
                    if result.is_err() {
                        Err(e) // Return original streaming error
                    } else {
                        result
                    }
                }
            }
        } else {
            let spinner = start_spinner("Generating code...");
            let result = api::generate_code_with_history(&conversation_history, config).await;
            stop_spinner(&spinner);
            result
        };

        match api_result {
            Ok(raw_response) => {
                // Log the response
                let _ = logger.log_api_response(&raw_response);

                // Extract clean Python code from the response
                let code = extract_python_code(&raw_response);
                last_generated_code = code.clone();

                // Add assistant response to history
                conversation_history.push(Message {
                    role: "assistant".to_string(),
                    content: code.clone(),
                });

                // Trim history to configured limit
                trim_history(&mut conversation_history, config.max_history_messages);

                display_code(&code);

                // Write the script first, then syntax-check before executing
                let script_path = match executor.write_script(&code) {
                    Ok(p) => p,
                    Err(e) => {
                        println!("{} {}", "✗ Failed to write script:".red(), e);
                        continue;
                    }
                };

                // Sync state to dashboard and broadcast event
                if let Some(ref ds) = dashboard {
                    sync_to_dashboard(
                        ds,
                        &metrics,
                        &last_synced_metrics,
                        &conversation_history,
                        &last_generated_code,
                    )
                    .await;
                    last_synced_metrics = metrics.clone();
                    ds.broadcast(ExecutionEvent::CodeGenerated {
                        code: code.clone(),
                        script_path: script_path.display().to_string(),
                    });
                }

                // Syntax check
                if let Err(syntax_err) = executor.syntax_check(&script_path) {
                    println!(
                        "\n{} {}",
                        "✗ Syntax error detected:".red().bold(),
                        syntax_err
                    );
                    if confirm("Auto-refine to fix this error?") {
                        // Add syntax error to conversation history for auto-refine
                        conversation_history.push(Message {
                            role: "user".to_string(),
                            content: format!(
                                "The code has a syntax error. Please fix it:\n{}",
                                syntax_err
                            ),
                        });
                        // Skip execution, let the loop iterate to call the API again
                        // by falling through (we already pushed the user message)
                        metrics.total_requests += 1;
                        let _ =
                            logger.log_api_request(&format!("Auto-refine syntax: {}", syntax_err));

                        let spinner = start_spinner("Auto-refining code...");
                        let api_result =
                            api::generate_code_with_history(&conversation_history, config).await;
                        stop_spinner(&spinner);

                        match api_result {
                            Ok(raw_response) => {
                                let _ = logger.log_api_response(&raw_response);
                                let fixed_code = extract_python_code(&raw_response);
                                last_generated_code = fixed_code.clone();

                                conversation_history.push(Message {
                                    role: "assistant".to_string(),
                                    content: fixed_code.clone(),
                                });
                                trim_history(
                                    &mut conversation_history,
                                    config.max_history_messages,
                                );

                                display_code(&fixed_code);

                                // Overwrite the script with the fixed code
                                if let Err(e) = fs::write(&script_path, &fixed_code) {
                                    println!("{} {}", "✗ Failed to write fixed script:".red(), e);
                                    continue;
                                }

                                // Re-check syntax
                                if let Err(err2) = executor.syntax_check(&script_path) {
                                    println!("{} {}", "✗ Still has syntax errors:".red(), err2);
                                    continue;
                                }
                            }
                            Err(e) => {
                                metrics.api_errors += 1;
                                let _ = logger
                                    .log_error(&format!("API error during auto-refine: {}", e));
                                println!("{} {}", "✗ API error during auto-refine:".red(), e);
                                conversation_history.pop();
                                continue;
                            }
                        }
                    } else {
                        continue;
                    }
                }

                // Run lint check (ruff) if available
                if linter_available {
                    match executor.lint_check(&script_path) {
                        Ok(lint_result) => {
                            display_lint_results(&lint_result);
                            if lint_result.has_errors {
                                if confirm("Auto-refine to fix lint errors?") {
                                    // Build a lint error summary for the LLM
                                    let lint_issues: String = lint_result
                                        .diagnostics
                                        .iter()
                                        .map(|d| d.message.as_str())
                                        .collect::<Vec<_>>()
                                        .join("\n");
                                    conversation_history.push(Message {
                                        role: "user".to_string(),
                                        content: format!(
                                            "The code has the following lint issues (from ruff). Please fix them:\n{}",
                                            lint_issues
                                        ),
                                    });
                                    metrics.total_requests += 1;
                                    let _ = logger.log_api_request(&format!(
                                        "Auto-refine lint: {}",
                                        lint_issues
                                    ));

                                    let spinner = start_spinner("Auto-refining code...");
                                    let api_result = api::generate_code_with_history(
                                        &conversation_history,
                                        config,
                                    )
                                    .await;
                                    stop_spinner(&spinner);

                                    match api_result {
                                        Ok(raw_response) => {
                                            let _ = logger.log_api_response(&raw_response);
                                            let fixed_code = extract_python_code(&raw_response);
                                            last_generated_code = fixed_code.clone();

                                            conversation_history.push(Message {
                                                role: "assistant".to_string(),
                                                content: fixed_code.clone(),
                                            });
                                            trim_history(
                                                &mut conversation_history,
                                                config.max_history_messages,
                                            );

                                            display_code(&fixed_code);

                                            if let Err(e) = fs::write(&script_path, &fixed_code) {
                                                println!(
                                                    "{} {}",
                                                    "✗ Failed to write fixed script:".red(),
                                                    e
                                                );
                                                continue;
                                            }

                                            // Re-check syntax after lint fix
                                            if let Err(syn_err) =
                                                executor.syntax_check(&script_path)
                                            {
                                                println!(
                                                    "{} {}",
                                                    "✗ Fixed code has syntax errors:".red(),
                                                    syn_err
                                                );
                                                continue;
                                            }
                                        }
                                        Err(e) => {
                                            metrics.api_errors += 1;
                                            let _ = logger.log_error(&format!(
                                                "API error during lint auto-refine: {}",
                                                e
                                            ));
                                            println!(
                                                "{} {}",
                                                "✗ API error during auto-refine:".red(),
                                                e
                                            );
                                            conversation_history.pop();
                                            continue;
                                        }
                                    }
                                } else if !confirm("Proceed with execution despite lint errors?") {
                                    continue;
                                }
                            }
                        }
                        Err(e) => {
                            println!("{} {}", "⚠️  Lint check failed:".yellow(), e);
                            println!("{}", "Proceeding without linting...".dimmed());
                        }
                    }
                }

                // Run security check (bandit) if available
                if security_scanner_available {
                    match executor.security_check(&script_path) {
                        Ok(sec_result) => {
                            display_security_results(&sec_result);
                            if sec_result.has_high_severity
                                && !confirm("HIGH severity security issues found. Proceed anyway?")
                            {
                                continue;
                            }
                        }
                        Err(e) => {
                            println!("{} {}", "⚠️  Security scan failed:".yellow(), e);
                            println!("{}", "Proceeding without security scanning...".dimmed());
                        }
                    }
                }

                if confirm("Execute this script?") {
                    // Create a venv for this execution (host mode only)
                    let venv = executor.create_venv().unwrap_or_else(|e| {
                        println!("{} {}", "⚠️  Failed to create venv:".yellow(), e);
                        println!("{}", "Proceeding without virtual environment...".dimmed());
                        None
                    });

                    // Check for dependencies
                    let deps = executor.detect_dependencies(&last_generated_code);
                    if !deps.is_empty() {
                        println!(
                            "\n{} {}",
                            "⚠️  Detected non-standard dependencies:".yellow(),
                            deps.join(", ").bright_yellow()
                        );
                        if config.auto_install_deps || confirm("Install these dependencies?") {
                            if let Err(e) = executor.install_packages(&deps, venv.as_deref()) {
                                println!(
                                    "{} {}",
                                    "⚠️  Failed to install dependencies:".yellow(),
                                    e
                                );
                                println!("{}", "Proceeding anyway...".dimmed());
                            }
                        }
                    }

                    // Detect if interactive mode is needed
                    let mode = if executor.needs_interactive_mode(&last_generated_code) {
                        println!(
                            "{}",
                            "🎮 Interactive mode detected (pygame/input/GUI)"
                                .bright_magenta()
                                .bold()
                        );
                        println!(
                            "{}",
                            "   Running with inherited stdio for user interaction...".dimmed()
                        );
                        ExecutionMode::Interactive
                    } else {
                        ExecutionMode::Captured
                    };

                    // Broadcast execution start to dashboard
                    if let Some(ref ds) = dashboard {
                        ds.broadcast(ExecutionEvent::ExecutionStarted {
                            script_path: script_path.display().to_string(),
                        });
                    }

                    match executor.execute_script(
                        &script_path,
                        mode,
                        config.execution_timeout_secs,
                        venv.as_deref(),
                        &deps,
                    ) {
                        Ok(result) => {
                            let success = result.is_success();
                            if success {
                                metrics.successful_executions += 1;
                            } else {
                                metrics.failed_executions += 1;
                            }

                            let _ = logger.log_execution(success, &result.stdout);

                            // Broadcast execution result to dashboard
                            if let Some(ref ds) = dashboard {
                                broadcast_execution_output(ds, &result.stdout, &result.stderr);
                                ds.broadcast(ExecutionEvent::ExecutionCompleted {
                                    success,
                                    exit_code: result.exit_code,
                                });
                                sync_to_dashboard(
                                    ds,
                                    &metrics,
                                    &last_synced_metrics,
                                    &conversation_history,
                                    &last_generated_code,
                                )
                                .await;
                                last_synced_metrics = metrics.clone();
                            }

                            println!(
                                "\n{}",
                                "━━━━━━━━━━━ Execution Result ━━━━━━━━━━━"
                                    .bright_blue()
                                    .bold()
                            );
                            println!("{} {:?}", "Script saved at:".dimmed(), result.script_path);
                            if !result.stdout.is_empty() {
                                println!("\n{}:", "STDOUT".green().bold());
                                println!("{}", result.stdout);
                            }
                            if !result.stderr.is_empty() {
                                println!("\n{}:", "STDERR".red().bold());
                                println!("{}", result.stderr);
                            }
                            println!(
                                "{}",
                                "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_blue()
                            );

                            // Offer auto-refine on runtime errors
                            if !success
                                && !result.stderr.is_empty()
                                && confirm("Auto-refine to fix this runtime error?")
                            {
                                conversation_history.push(Message {
                                    role: "user".to_string(),
                                    content: format!(
                                        "The code crashed with this runtime error. Please fix it:\n{}",
                                        result.stderr
                                    ),
                                });
                                metrics.total_requests += 1;
                                let _ = logger.log_api_request(&format!(
                                    "Auto-refine runtime: {}",
                                    result.stderr
                                ));

                                let spinner = start_spinner("Auto-refining code...");
                                let api_result =
                                    api::generate_code_with_history(&conversation_history, config)
                                        .await;
                                stop_spinner(&spinner);

                                match api_result {
                                    Ok(raw_response) => {
                                        let _ = logger.log_api_response(&raw_response);
                                        let fixed_code = extract_python_code(&raw_response);
                                        last_generated_code = fixed_code.clone();

                                        conversation_history.push(Message {
                                            role: "assistant".to_string(),
                                            content: fixed_code.clone(),
                                        });
                                        trim_history(
                                            &mut conversation_history,
                                            config.max_history_messages,
                                        );

                                        display_code(&fixed_code);

                                        // Detect updated deps for the fixed code
                                        let fixed_deps = executor.detect_dependencies(&fixed_code);

                                        // Overwrite the script with the fixed code
                                        if let Err(e) = fs::write(&script_path, &fixed_code) {
                                            println!(
                                                "{} {}",
                                                "✗ Failed to write fixed script:".red(),
                                                e
                                            );
                                        } else if let Err(syn_err) =
                                            executor.syntax_check(&script_path)
                                        {
                                            println!(
                                                "{} {}",
                                                "✗ Fixed code has syntax errors:".red(),
                                                syn_err
                                            );
                                        } else if confirm("Execute the fixed script?") {
                                            // Reuse the same venv for the retry execution
                                            match executor.execute_script(
                                                &script_path,
                                                mode,
                                                config.execution_timeout_secs,
                                                venv.as_deref(),
                                                &fixed_deps,
                                            ) {
                                                Ok(retry_result) => {
                                                    let retry_success = retry_result.is_success();
                                                    if retry_success {
                                                        metrics.successful_executions += 1;
                                                    } else {
                                                        metrics.failed_executions += 1;
                                                    }
                                                    let _ = logger.log_execution(
                                                        retry_success,
                                                        &retry_result.stdout,
                                                    );

                                                    println!(
                                                        "\n{}",
                                                        "━━━━━━━━━━━ Execution Result ━━━━━━━━━━━"
                                                            .bright_blue()
                                                            .bold()
                                                    );
                                                    println!(
                                                        "{} {:?}",
                                                        "Script saved at:".dimmed(),
                                                        retry_result.script_path
                                                    );
                                                    if !retry_result.stdout.is_empty() {
                                                        println!("\n{}:", "STDOUT".green().bold());
                                                        println!("{}", retry_result.stdout);
                                                    }
                                                    if !retry_result.stderr.is_empty() {
                                                        println!("\n{}:", "STDERR".red().bold());
                                                        println!("{}", retry_result.stderr);
                                                    }
                                                    println!(
                                                        "{}",
                                                        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
                                                            .bright_blue()
                                                    );
                                                }
                                                Err(e) => {
                                                    metrics.failed_executions += 1;
                                                    let _ = logger.log_error(&format!(
                                                        "Execution error: {}",
                                                        e
                                                    ));
                                                    println!(
                                                        "{} {}",
                                                        "✗ Execution error:".red(),
                                                        e
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        metrics.api_errors += 1;
                                        let _ = logger.log_error(&format!(
                                            "API error during auto-refine: {}",
                                            e
                                        ));
                                        println!(
                                            "{} {}",
                                            "✗ API error during auto-refine:".red(),
                                            e
                                        );
                                        conversation_history.pop();
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            metrics.failed_executions += 1;
                            let _ = logger.log_error(&format!("Execution error: {}", e));
                            println!("{} {}", "✗ Execution error:".red(), e);
                        }
                    }

                    // Clean up the venv after execution is done
                    if let Some(ref venv_path) = venv {
                        executor.cleanup_venv(venv_path);
                    }
                }
            }
            Err(e) => {
                metrics.api_errors += 1;
                let _ = logger.log_error(&format!("API error: {}", e));
                println!("{} {}", "✗ API error:".red(), e);
                // Remove the last user message if API call failed
                conversation_history.pop();
            }
        }
    }

    // Display session statistics on exit
    println!("\n{}", "Session ended.".bright_cyan());
    metrics.display();
}

/// Sync local REPL state to the shared dashboard state.
///
/// Uses delta-based merging for metrics so that dashboard-originated
/// metrics (from /api/generate) are not overwritten by the REPL sync.
async fn sync_to_dashboard(
    ds: &Arc<DashboardState>,
    metrics: &SessionMetrics,
    last_synced: &SessionMetrics,
    history: &[Message],
    last_code: &str,
) {
    {
        let mut m = ds.metrics.write().await;
        m.total_requests += metrics
            .total_requests
            .saturating_sub(last_synced.total_requests);
        m.successful_executions += metrics
            .successful_executions
            .saturating_sub(last_synced.successful_executions);
        m.failed_executions += metrics
            .failed_executions
            .saturating_sub(last_synced.failed_executions);
        m.api_errors += metrics.api_errors.saturating_sub(last_synced.api_errors);
    }
    {
        let mut h = ds.conversation_history.write().await;
        *h = history.to_vec();
    }
    {
        let mut c = ds.last_generated_code.write().await;
        *c = last_code.to_string();
    }
}

/// Send stdout and stderr lines as individual log events to the dashboard.
fn broadcast_execution_output(ds: &Arc<DashboardState>, stdout: &str, stderr: &str) {
    let ts = chrono::Local::now().format("%H:%M:%S").to_string();
    for line in stdout.lines() {
        ds.broadcast(ExecutionEvent::LogLine {
            timestamp: ts.clone(),
            stream: "stdout".to_string(),
            content: line.to_string(),
        });
    }
    for line in stderr.lines() {
        ds.broadcast(ExecutionEvent::LogLine {
            timestamp: ts.clone(),
            stream: "stderr".to_string(),
            content: line.to_string(),
        });
    }
}

/// Display lint results with colored output.
fn display_lint_results(result: &crate::python_exec::LintResult) {
    if result.passed {
        println!("{}", "✓ Lint check passed — no issues found.".green());
        return;
    }

    println!(
        "\n{}",
        "━━━━━━━━━━━━ Lint Results ━━━━━━━━━━━━"
            .bright_yellow()
            .bold()
    );
    for diag in &result.diagnostics {
        let icon = match diag.severity {
            LintSeverity::Error => "  ✗".red().bold(),
            LintSeverity::Warning => "  ⚠".yellow(),
        };
        println!("{} {}", icon, diag.message);
    }
    if !result.summary.is_empty() {
        println!("\n{}", result.summary.dimmed());
    }
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_yellow()
    );
}

/// Display security scan results with colored output.
fn display_security_results(result: &crate::python_exec::SecurityResult) {
    if result.passed {
        println!("{}", "✓ Security scan passed — no issues found.".green());
        return;
    }

    println!(
        "\n{}",
        "━━━━━━━━━━ Security Scan Results ━━━━━━━━━━"
            .bright_red()
            .bold()
    );
    for diag in &result.diagnostics {
        let icon = match diag.severity {
            SecuritySeverity::High => "  ✗".red().bold(),
            SecuritySeverity::Medium => "  ⚠".yellow(),
            SecuritySeverity::Low => "  ℹ".dimmed(),
        };
        let sev_label = match diag.severity {
            SecuritySeverity::High => format!("[{}]", diag.severity).red().bold().to_string(),
            SecuritySeverity::Medium => format!("[{}]", diag.severity).yellow().to_string(),
            SecuritySeverity::Low => format!("[{}]", diag.severity).dimmed().to_string(),
        };
        println!("{} {} {}", icon, sev_label, diag.message);
    }
    if !result.summary.is_empty() {
        println!("\n{}", result.summary.dimmed());
    }
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_red()
    );
}
