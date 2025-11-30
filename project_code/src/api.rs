use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const HUGGINGFACE_BASE: &str = "https://api-inference.huggingface.co/models";

#[derive(Serialize)] //pour ecrire la requette json
struct HfRequest<'a> {  //pour que la variable a continue d'exister assez longtemps pour faire la requette
    inputs: &'a str,  //en gros la requette qu'on envoit
    #[serde(skip_serializing_if = "Option::is_none")] //si on veux ajouter des paramètre
    parameters: Option<HfParameters>,  //les paramètres qu'on veux ajouter (truc d'apres)
}


#[derive(Serialize)]//aussi pour ecrire le json
struct HfParameters { //pour mettre les options
    max_new_tokens: Option<u32>,//nb de token que le model peut generé en plus: plus il est grand plus la reponse sera longue
    temperature: Option<f32>,//creativité du model: 0 tres deterministe bien pour le code
    // ajoute d'autres paramètres si besoin
}

#[derive(Debug, Deserialize)] //pour recuperer la réponse: peut y avoir plusieurs formats donc plusieurs options dans ce code pour s'adapter
//deserialisable pour passer de json a rust, serialisable pour passer de rust a json
struct HfGenerated {
    
    #[serde(rename = "generated_text")] //on cherche a recuperer le champs generated text car c'est la que se trouve la reponse
    generated_text: Option<String>,

    #[serde(rename = "text")] //desfois c'est le champs text
    text: Option<String>,

    //rajouter si on tombe sur des cas ou la reponse se trouve dans un autre champs
}
