use std::io::{self, Write};
use crate::api::{self, Message};
use crate::python_exec::CodeExecutor;
use crate::utils::extract_python_code;

// Fonction publique utilisable depuis main.rs affichant un bandeau de bienvenue 
pub fn print_banner() {
    println!("====================================");
    println!("        PYTHON MAKER BOT v0.1       ");
    println!("====================================");
    println!(" Write your idea or /quit to exit\n");
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
    println!("----------- Generated code -----------");
    println!("{code}");
    println!("-----------------------------------\n");
}

// Boucle interactive : affiche le bandeau de lancement
pub async fn start_repl() {
    print_banner();

    let executor = CodeExecutor::new("generated").expect("Impossible de créer le dossier");
    
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
            println!("Available commands:");
            println!("  /quit, /exit  - Exit the program");
            println!("  /help         - Show this help");
            println!("  /clear        - Clear conversation history");
            println!("  /refine       - Refine the last generated code");
            continue;
        }

        if prompt == "/clear" {
            conversation_history.clear();
            last_generated_code.clear();
            println!("Conversation history cleared.");
            continue;
        }

        if prompt == "/refine" {
            if last_generated_code.is_empty() {
                println!("No code to refine. Generate some code first!");
                continue;
            }
            let refinement = ask_user("What would you like to change or add? ");
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

        // Call Hugging Face with conversation history
        match api::generate_code_with_history(conversation_history.clone()).await {
            Ok(raw_response) => {
                // Extract clean Python code from the response
                let code = extract_python_code(&raw_response);
                last_generated_code = code.clone();
                
                // Add assistant response to history
                conversation_history.push(Message {
                    role: "assistant".to_string(),
                    content: code.clone(),
                });
                
                display_code(&code);

                if confirm("Execute this script?") {
                    match executor.write_and_run(&code) {
                        Ok(result) => {
                            println!("--- Result ---");
                            println!("Script saved at: {:?}", result.script_path);
                            println!("STDOUT:\n{}", result.stdout);
                            if !result.stderr.is_empty() {
                                println!("STDERR:\n{}", result.stderr);
                            }
                        }
                        Err(e) => println!("Execution error: {}", e),
                    }
                }
            }
            Err(e) => {
                println!("API error: {}", e);
                // Remove the last user message if API call failed
                conversation_history.pop();
            }
        }
    }
}
