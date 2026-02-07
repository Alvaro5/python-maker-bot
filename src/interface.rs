use std::io::{self, Write};
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use crate::api::{self, Message, Provider};
use crate::config::AppConfig;
use crate::python_exec::{CodeExecutor, ExecutionMode, LintSeverity};
use crate::utils::{extract_python_code, find_char_boundary};
use crate::logger::{Logger, SessionMetrics};
use colored::*;
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::hint::Hinter;
use rustyline::{Config, CompletionType, Context, Editor, Helper, Highlighter, Validator};

/// Available slash commands for tab-completion.
const COMMANDS: &[&str] = &[
    "/help", "/quit", "/exit", "/clear", "/refine",
    "/save", "/history", "/stats", "/list", "/run", "/provider", "/lint",
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

// Fonction publique utilisable depuis main.rs affichant un bandeau de bienvenue
pub fn print_banner() {
    println!("{}", "====================================".bright_cyan());
    println!("{}", "      PYTHON MAKER BOT v0.2.1       ".bright_cyan().bold());
    println!("{}", "====================================".bright_cyan());
    println!("{}", " AI-Powered Python Code Generator".bright_white());
    println!("{}\n", " Type /help for commands or /quit to exit".dimmed());
}

// Utility function to ask the user a question and return their answer
pub fn ask_user(question: &str) -> String {
    print!("{question}");
    io::stdout().flush().unwrap();

    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    input.trim().to_string()
}

// Utility function that asks a yes/no question using ask_user
pub fn confirm(question: &str) -> bool {
    let ans = ask_user(&format!("{question} (y/n) : "));
    ans.to_lowercase().starts_with('y')
}

// Display function for generated Python code
pub fn display_code(code: &str) {
    println!("\n{}", "━━━━━━━━━━━ Generated Code ━━━━━━━━━━━".bright_green().bold());
    // Simple syntax highlighting for Python
    for line in code.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            println!("{}", line.bright_black());
        } else if trimmed.starts_with("def ") || trimmed.starts_with("class ") {
            println!("{}", line.bright_yellow());
        } else if trimmed.starts_with("import ") || trimmed.starts_with("from ") {
            println!("{}", line.bright_magenta());
        } else {
            println!("{}", line);
        }
    }
    println!("{}\n", "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_green());
}

/// Trim conversation history to at most `max` messages, dropping the oldest
/// user/assistant pairs first.
fn trim_history(history: &mut Vec<Message>, max: usize) {
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
            print!("\r{} {} ", frames[i % frames.len()].to_string().cyan(), msg.dimmed());
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

// Interactive REPL entry point
pub async fn start_repl(config: &AppConfig) {
    print_banner();

    // Validate and display the configured provider
    let provider = match Provider::from_config(&config.provider) {
        Ok(p) => p,
        Err(e) => {
            println!("{} {}", "✗ Invalid provider configuration:".red().bold(), e);
            return;
        }
    };
    match provider.resolve_api_url(&config.api_url) {
        Ok(url) => println!("{} {} → {}", "✓ Provider:".green(), provider.display_name().bright_white(), url.dimmed()),
        Err(e) => {
            println!("{} {}", "✗ Provider configuration error:".red().bold(), e);
            return;
        }
    }

    let executor = CodeExecutor::new(&config.generated_dir, config.use_docker, config.use_venv, &config.python_executable).expect("Failed to create generated scripts directory");
    let logger = Logger::new(&config.log_dir).expect("Failed to create logger");
    let metrics = SessionMetrics::new();

    if config.use_venv {
        println!("{}", "✓ Virtual environment isolation enabled.".green());
    }

    // Check linter availability
    let linter_available = if config.use_linting {
        if CodeExecutor::check_linter_available() {
            println!("{}", "✓ Linting enabled (ruff detected).".green());
            true
        } else {
            println!("{}", "⚠️  Linting enabled but ruff not found. Install with: pip install ruff".yellow());
            println!("{}", "  Linting will be skipped until ruff is installed.".dimmed());
            false
        }
    } else {
        false
    };

    // If Docker mode is enabled, verify Docker is available
    if config.use_docker {
        match CodeExecutor::check_docker_available() {
            Ok(()) => println!("{}", "✓ Docker sandbox mode enabled.".green()),
            Err(e) => {
                println!("{} {}", "✗ Docker sandbox not available:".red().bold(), e);
                println!("{}", "  Falling back to host execution.".yellow());
                println!("{}", "  To enable Docker, run: docker build -t python-sandbox .".dimmed());
                // Recreate executor without Docker
                // (we can't mutate executor, so shadow it)
                let executor = CodeExecutor::new(&config.generated_dir, false, config.use_venv, &config.python_executable).expect("Failed to create generated scripts directory");
                return start_repl_loop(config, executor, logger, metrics, linter_available).await;
            }
        }
    }

    start_repl_loop(config, executor, logger, metrics, linter_available).await;
}

async fn start_repl_loop(
    config: &AppConfig,
    executor: CodeExecutor,
    logger: Logger,
    mut metrics: SessionMetrics,
    linter_available: bool,
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

    loop {
        let readline = rl.readline(&"> ".bright_cyan().bold().to_string());
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
            println!("\n{}", "Available Commands:".bright_cyan().bold());
            println!("  {}  - Exit the program", "/quit, /exit".green());
            println!("  {}         - Show this help", "/help".green());
            println!("  {}        - Clear conversation history", "/clear".green());
            println!("  {}       - Refine the last generated code", "/refine".green());
            println!("  {} <file> - Save last code to a file", "/save".green());
            println!("  {}      - Show conversation history", "/history".green());
            println!("  {}        - Show session statistics", "/stats".green());
            println!("  {}         - List all generated scripts", "/list".green());
            println!("  {} <file>  - Execute a previously generated script", "/run".green());
            println!("  {}     - Show current LLM provider info", "/provider".green());
            println!("  {}         - Lint the last generated code with ruff", "/lint".green());
            println!();
            continue;
        }

        if prompt == "/stats" {
            metrics.display();
            continue;
        }

        if prompt == "/provider" {
            if let Ok(p) = Provider::from_config(&config.provider) {
                println!("\n{}", "LLM Provider Info:".bright_cyan().bold());
                println!("  {} {}", "Provider:".dimmed(), p.display_name().bright_white());
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
                println!("{}", "Linter (ruff) is not available. Install with: pip install ruff".yellow());
                continue;
            }
            // Write to a temp file for linting
            match executor.write_script(&last_generated_code) {
                Ok(path) => {
                    match executor.lint_check(&path) {
                        Ok(lint_result) => display_lint_results(&lint_result),
                        Err(e) => println!("{} {}", "✗ Lint error:".red(), e),
                    }
                }
                Err(e) => println!("{} {}", "✗ Failed to write script for linting:".red(), e),
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
                println!("\n{}", "Conversation History:".bright_cyan().bold());
                for (i, msg) in conversation_history.iter().enumerate() {
                    let role_color = if msg.role == "user" {
                        msg.role.bright_blue()
                    } else {
                        msg.role.bright_green()
                    };
                    println!("\n{}. [{}]", i + 1, role_color);
                    let preview = if msg.content.len() > 100 {
                        let end = find_char_boundary(&msg.content, 100);
                        format!("{}...", &msg.content[..end])
                    } else {
                        msg.content.clone()
                    };
                    println!("{}", preview.dimmed());
                }
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
                        println!("\n{}", "Generated Scripts:".bright_cyan().bold());
                        for (i, entry) in scripts.iter().enumerate() {
                            println!("  {}. {}", i + 1, entry.file_name().to_string_lossy().bright_white());
                        }
                        println!();
                    }
                }
                Err(e) => println!("{} {}", "✗ Failed to list scripts:".red(), e),
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
                        println!("\n{} {}",
                            "⚠️  Detected non-standard dependencies:".yellow(),
                            deps.join(", ").bright_yellow());
                        if config.auto_install_deps || confirm("Install these dependencies?") {
                            if let Err(e) = executor.install_packages(&deps, venv.as_deref()) {
                                println!("{} {}", "⚠️  Failed to install dependencies:".yellow(), e);
                                println!("{}", "Proceeding anyway...".dimmed());
                            }
                        }
                    }

                    // Detect if interactive mode is needed
                    let mode = if executor.needs_interactive_mode(&code) {
                        println!("{}", "🎮 Interactive mode detected (pygame/input/GUI)".bright_magenta().bold());
                        println!("{}", "   Running with inherited stdio for user interaction...".dimmed());
                        ExecutionMode::Interactive
                    } else {
                        ExecutionMode::Captured
                    };

                    match executor.run_existing_script(&script_path, mode, config.execution_timeout_secs, venv.as_deref(), &deps) {
                        Ok(result) => {
                            let success = result.is_success();
                            if success {
                                metrics.successful_executions += 1;
                            } else {
                                metrics.failed_executions += 1;
                            }

                            let _ = logger.log_execution(success, &result.stdout);

                            println!("\n{}", "━━━━━━━━━━━ Execution Result ━━━━━━━━━━━".bright_blue().bold());
                            if !result.stdout.is_empty() {
                                println!("\n{}:", "STDOUT".green().bold());
                                println!("{}", result.stdout);
                            }
                            if !result.stderr.is_empty() {
                                println!("\n{}:", "STDERR".red().bold());
                                println!("{}", result.stderr);
                            }
                            println!("{}", "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_blue());
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

        if prompt == "/refine" {
            if last_generated_code.is_empty() {
                println!("{}", "No code to refine. Generate some code first!".yellow());
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

            // Add refinement request to history
            conversation_history.push(Message {
                role: "user".to_string(),
                content: format!("Please refine the previous code: {}", refinement),
            });
        } else {
            // Regular prompt - add to history
            conversation_history.push(Message {
                role: "user".to_string(),
                content: prompt.clone(),
            });
        }

        // Log the request
        let _ = logger.log_api_request(&conversation_history.last().unwrap().content);
        metrics.total_requests += 1;

        // Call Hugging Face with conversation history
        let spinner = start_spinner("Generating code...");
        let api_result = api::generate_code_with_history(conversation_history.clone(), config).await;
        stop_spinner(&spinner);

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

                // Syntax check
                if let Err(syntax_err) = executor.syntax_check(&script_path) {
                    println!("\n{} {}", "✗ Syntax error detected:".red().bold(), syntax_err);
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
                        let _ = logger.log_api_request(&format!("Auto-refine syntax: {}", syntax_err));

                        let spinner = start_spinner("Auto-refining code...");
                        let api_result = api::generate_code_with_history(conversation_history.clone(), config).await;
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
                                trim_history(&mut conversation_history, config.max_history_messages);

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
                                let _ = logger.log_error(&format!("API error during auto-refine: {}", e));
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
                                    let lint_issues: String = lint_result.diagnostics
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
                                    let _ = logger.log_api_request(&format!("Auto-refine lint: {}", lint_issues));

                                    let spinner = start_spinner("Auto-refining code...");
                                    let api_result = api::generate_code_with_history(conversation_history.clone(), config).await;
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
                                            trim_history(&mut conversation_history, config.max_history_messages);

                                            display_code(&fixed_code);

                                            if let Err(e) = fs::write(&script_path, &fixed_code) {
                                                println!("{} {}", "✗ Failed to write fixed script:".red(), e);
                                                continue;
                                            }

                                            // Re-check syntax after lint fix
                                            if let Err(syn_err) = executor.syntax_check(&script_path) {
                                                println!("{} {}", "✗ Fixed code has syntax errors:".red(), syn_err);
                                                continue;
                                            }
                                        }
                                        Err(e) => {
                                            metrics.api_errors += 1;
                                            let _ = logger.log_error(&format!("API error during lint auto-refine: {}", e));
                                            println!("{} {}", "✗ API error during auto-refine:".red(), e);
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
                        println!("\n{} {}",
                            "⚠️  Detected non-standard dependencies:".yellow(),
                            deps.join(", ").bright_yellow());
                        if config.auto_install_deps || confirm("Install these dependencies?") {
                            if let Err(e) = executor.install_packages(&deps, venv.as_deref()) {
                                println!("{} {}", "⚠️  Failed to install dependencies:".yellow(), e);
                                println!("{}", "Proceeding anyway...".dimmed());
                            }
                        }
                    }

                    // Detect if interactive mode is needed
                    let mode = if executor.needs_interactive_mode(&last_generated_code) {
                        println!("{}", "🎮 Interactive mode detected (pygame/input/GUI)".bright_magenta().bold());
                        println!("{}", "   Running with inherited stdio for user interaction...".dimmed());
                        ExecutionMode::Interactive
                    } else {
                        ExecutionMode::Captured
                    };

                    match executor.execute_script(&script_path, mode, config.execution_timeout_secs, venv.as_deref(), &deps) {
                        Ok(result) => {
                            let success = result.is_success();
                            if success {
                                metrics.successful_executions += 1;
                            } else {
                                metrics.failed_executions += 1;
                            }

                            let _ = logger.log_execution(success, &result.stdout);

                            println!("\n{}", "━━━━━━━━━━━ Execution Result ━━━━━━━━━━━".bright_blue().bold());
                            println!("{} {:?}", "Script saved at:".dimmed(), result.script_path);
                            if !result.stdout.is_empty() {
                                println!("\n{}:", "STDOUT".green().bold());
                                println!("{}", result.stdout);
                            }
                            if !result.stderr.is_empty() {
                                println!("\n{}:", "STDERR".red().bold());
                                println!("{}", result.stderr);
                            }
                            println!("{}", "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_blue());

                            // Offer auto-refine on runtime errors
                            if !success && !result.stderr.is_empty()
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
                                let _ = logger.log_api_request(&format!("Auto-refine runtime: {}", result.stderr));

                                let spinner = start_spinner("Auto-refining code...");
                                let api_result = api::generate_code_with_history(conversation_history.clone(), config).await;
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
                                        trim_history(&mut conversation_history, config.max_history_messages);

                                        display_code(&fixed_code);

                                        // Detect updated deps for the fixed code
                                        let fixed_deps = executor.detect_dependencies(&fixed_code);

                                        // Overwrite the script with the fixed code
                                        if let Err(e) = fs::write(&script_path, &fixed_code) {
                                            println!("{} {}", "✗ Failed to write fixed script:".red(), e);
                                        } else if let Err(syn_err) = executor.syntax_check(&script_path) {
                                            println!("{} {}", "✗ Fixed code has syntax errors:".red(), syn_err);
                                        } else if confirm("Execute the fixed script?") {
                                            // Reuse the same venv for the retry execution
                                            match executor.execute_script(&script_path, mode, config.execution_timeout_secs, venv.as_deref(), &fixed_deps) {
                                                Ok(retry_result) => {
                                                    let retry_success = retry_result.is_success();
                                                    if retry_success {
                                                        metrics.successful_executions += 1;
                                                    } else {
                                                        metrics.failed_executions += 1;
                                                    }
                                                    let _ = logger.log_execution(retry_success, &retry_result.stdout);

                                                    println!("\n{}", "━━━━━━━━━━━ Execution Result ━━━━━━━━━━━".bright_blue().bold());
                                                    println!("{} {:?}", "Script saved at:".dimmed(), retry_result.script_path);
                                                    if !retry_result.stdout.is_empty() {
                                                        println!("\n{}:", "STDOUT".green().bold());
                                                        println!("{}", retry_result.stdout);
                                                    }
                                                    if !retry_result.stderr.is_empty() {
                                                        println!("\n{}:", "STDERR".red().bold());
                                                        println!("{}", retry_result.stderr);
                                                    }
                                                    println!("{}", "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_blue());
                                                }
                                                Err(e) => {
                                                    metrics.failed_executions += 1;
                                                    let _ = logger.log_error(&format!("Execution error: {}", e));
                                                    println!("{} {}", "✗ Execution error:".red(), e);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        metrics.api_errors += 1;
                                        let _ = logger.log_error(&format!("API error during auto-refine: {}", e));
                                        println!("{} {}", "✗ API error during auto-refine:".red(), e);
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

/// Display lint results with colored output.
fn display_lint_results(result: &crate::python_exec::LintResult) {
    if result.passed {
        println!("{}", "✓ Lint check passed — no issues found.".green());
        return;
    }

    println!("\n{}", "━━━━━━━━━━━━ Lint Results ━━━━━━━━━━━━".bright_yellow().bold());
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
    println!("{}", "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_yellow());
}

