use std::io::{self, Write};
use std::fs;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};

/// Structure représentant la mémoire du chatbot.
/// Elle associe des phrases d'entrée à des réponses apprises.
#[derive(Serialize, Deserialize)]
struct Memory {
    responses: HashMap<String, String>,
}

impl Memory {
    /// Charge la mémoire depuis le fichier JSON, ou crée une mémoire vide si le fichier n'existe pas.
    fn load() -> Self {
        fs::read_to_string("memory.json")
            .ok()
            .and_then(|data| serde_json::from_str(&data).ok())
            .unwrap_or(Self { responses: HashMap::new() })
    }

    /// Sauvegarde la mémoire actuelle dans le fichier JSON.
    fn save(&self) {
        let _ = fs::write("memory.json", serde_json::to_string_pretty(self).unwrap());
    }
}

fn main() {
    println!("RustBot v0.3 – Chatbot avec mémoire persistante");
    println!("Tapez 'quit' pour quitter.\n");

    let mut memory = Memory::load();

    loop {
        print!("Vous : ");
        io::stdout().flush().unwrap();

        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        let input = input.trim().to_lowercase();

        if input == "quit" {
            println!("RustBot : Au revoir !");
            memory.save();
            break;
        }

        if let Some(reply) = memory.responses.get(&input) {
            println!("RustBot : {}", reply);
        } else {
            println!("RustBot : Je ne connais pas cette phrase. Que devrais-je répondre ?");
            print!("Vous : ");
            io::stdout().flush().unwrap();

            let mut new_reply = String::new();
            io::stdin().read_line(&mut new_reply).unwrap();
            let new_reply = new_reply.trim().to_string();

            memory.responses.insert(input.clone(), new_reply.clone());
            memory.save();

            println!("RustBot : Très bien, je répondrai désormais '{}' à '{}'.", new_reply, input);
        }
    }
}
