use std::io::{self, Write};

fn main() {
    println!("RustBot v0.1 – Chatbot minimal");
    println!("Tapez 'quit' pour quitter le programme.\n");

    loop {
        // Lecture de l'entrée utilisateur
        print!("Vous : ");
        io::stdout().flush().unwrap(); // forcer l'affichage immédiat du prompt

        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        let input = input.trim().to_lowercase();

        // Condition de sortie
        if input == "quit" {
            println!("RustBot : Au revoir !");
            break;
        }

        // Réponse générique
        println!("RustBot : Vous avez dit '{}'.", input);
    }
}
