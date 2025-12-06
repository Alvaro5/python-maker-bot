use std::io::{self, Write};
use crate::api;
use crate::python_exec::CodeExecutor;

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

    let executor = CodeExecutor::new("generated").expect("Impossible de créer le dossier"); // création d'un exécuteur qui va écrire les scripts python dans le dossier generated

    loop {
        let prompt = ask_user("> ");

        if prompt == "/quit" || prompt == "/exit" {
            println!("Goodbye!");
            break;
        }

        if prompt == "/help" {
            println!("Available commands: /quit /help");
            continue;
        }

        // Call Hugging Face
        match api::generate_code(&prompt).await {
            Ok(code) => {
                display_code(&code);

                if confirm("Execute this script?") {
                    match executor.write_and_run(&code) {
                        Ok(result) => {
                            println!("--- Result ---");
                            println!("Script saved at: {:?}", result.script_path);
                            println!("STDOUT:\n{}", result.stdout);
                            println!("STDERR:\n{}", result.stderr);
                        }
                        Err(e) => println!("Erreur d'exécution: {}", e),
                    }
                }
            }
            Err(e) => {
                println!("API error: {}", e);
            }
        }
    }
}
