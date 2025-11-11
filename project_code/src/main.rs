use std::io::{self, Write};

/// Retourne une réponse adaptée selon l'entrée utilisateur
fn get_response(input: &str) -> &str {
    match input {
        "bonjour" | "salut" => "Bonjour ! Comment allez-vous ?",
        "ça va" | "bien" => "Très bien, merci. Et vous ?",
        "mal" | "bof" => "Je suis désolé de l'apprendre. Souhaitez-vous en parler ?",
        "merci" => "Avec plaisir.",
        "qui es-tu" => "Je suis un chatbot local développé en Rust.",
        _ => "Je ne comprends pas encore cette phrase.",
    }
}

fn main() {
    println!("RustBot v0.2 – Chatbot avec moteur de règles");
    println!("Tapez 'quit' pour quitter.\n");

    loop {
        print!("Vous : ");
        io::stdout().flush().unwrap();

        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        let input = input.trim().to_lowercase();

        if input == "quit" {
            println!("RustBot : Au revoir !");
            break;
        }

        let response = get_response(&input);
        println!("RustBot : {}", response);
    }
}
