use std::io::{self, Write};
use std::fs;
use crate::api::{self, Message};
use crate::config::AppConfig;
use crate::python_exec::{CodeExecutor, ExecutionMode};
use crate::utils::{extract_python_code, find_char_boundary};
use crate::logger::{Logger, SessionMetrics};
use colored::*;

// Fonction publique utilisable depuis main.rs affichant un bandeau de bienvenue
pub fn print_banner() {
    println!("{}", "====================================".bright_cyan());
    println!("{}", "      PYTHON MAKER BOT v0.2.1       ".bright_cyan().bold());
    println!("{}", "====================================".bright_cyan());
    println!("{}", " AI-Powered Python Code Generator".bright_white());
    println!("{}\n", " Type /help for commands or /quit to exit".dimmed());
}

// Fonction utilitaire pour poser des question à l'utilisateur et récupérer la réponse
pub fn ask_user(question: &str) -> String {
    print!("{question}");
    io::stdout().flush().unwrap();

    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    input.trim().to_string()
}

// Fonction utilitaire qui pose une une question oui/non en utilisant ask_user
// Elle renvoi un booléen
pub fn confirm(question: &str) -> bool {
    let ans = ask_user(&format!("{question} (o/n) : "));
    ans.to_lowercase().starts_with('o')
}

// Fonction d'affichage pour le code python généré
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

// Boucle interactive : affiche le bandeau de lancement
pub async fn start_repl(config: &AppConfig) {
    print_banner();

    let executor = CodeExecutor::new(&config.generated_dir).expect("Impossible de créer le dossier");
    let logger = Logger::new(&config.log_dir).expect("Failed to create logger");
    let mut metrics = SessionMetrics::new();

    // Conversation history for multi-turn refinement
    let mut conversation_history: Vec<Message> = Vec::new();
    let mut last_generated_code = String::new();

    loop {
        let prompt = ask_user("> ");

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
            println!();
            continue;
        }

        if prompt == "/stats" {
            metrics.display();
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

                    // Check for dependencies
                    let deps = executor.detect_dependencies(&code);
                    if !deps.is_empty() {
                        println!("\n{} {}",
                            "⚠️  Detected non-standard dependencies:".yellow(),
                            deps.join(", ").bright_yellow());
                        if config.auto_install_deps || confirm("Install these dependencies?") {
                            if let Err(e) = executor.install_packages(&deps) {
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

                    match executor.run_existing_script(&script_path, mode, config.execution_timeout_secs) {
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
        match api::generate_code_with_history(conversation_history.clone(), config).await {
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

                        match api::generate_code_with_history(conversation_history.clone(), config).await {
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

                if confirm("Execute this script?") {
                    // Check for dependencies
                    let deps = executor.detect_dependencies(&last_generated_code);
                    if !deps.is_empty() {
                        println!("\n{} {}",
                            "⚠️  Detected non-standard dependencies:".yellow(),
                            deps.join(", ").bright_yellow());
                        if config.auto_install_deps || confirm("Install these dependencies?") {
                            if let Err(e) = executor.install_packages(&deps) {
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

                    match executor.execute_script(&script_path, mode, config.execution_timeout_secs) {
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

                                match api::generate_code_with_history(conversation_history.clone(), config).await {
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
                                        } else if let Err(syn_err) = executor.syntax_check(&script_path) {
                                            println!("{} {}", "✗ Fixed code has syntax errors:".red(), syn_err);
                                        } else if confirm("Execute the fixed script?") {
                                            match executor.execute_script(&script_path, mode, config.execution_timeout_secs) {
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
