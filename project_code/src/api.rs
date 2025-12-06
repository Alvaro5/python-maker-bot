use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    // Add more parameters as needed (e.g., top_p, stream)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
}

pub async fn generate_code(prompt: &str) -> Result<String> {
    // Load the token
    let token = std::env::var("HF_TOKEN")
        .context("HF_TOKEN missing in .env")?;

    // Set the router URL (OpenAI-compatible endpoint)
    let url = "https://router.huggingface.co/v1/chat/completions".to_string();

    // Build the request body
    let body = ChatRequest {
        model: "Qwen/Qwen2.5-Coder-7B-Instruct".to_string(),
        messages: vec![
            Message {
                role: "system".to_string(),
                content: "You are an expert Python code generator. Generate clean, well-commented, executable Python code based on user requests. \
                         Follow these rules:\n\
                         1. Output ONLY valid Python code\n\
                         2. Include helpful comments explaining the logic\n\
                         3. Use proper Python conventions and best practices\n\
                         4. Handle errors gracefully with try-except where appropriate\n\
                         5. If external libraries are needed, import them at the top\n\
                         6. Make the code production-ready and maintainable".to_string(),
            },
            Message {
                role: "user".to_string(),
                content: prompt.to_string(),
            },
        ],
        max_tokens: Some(1024),
        temperature: Some(0.2),
    };

    // Build headers
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}"))
            .context("Invalid Bearer token format")?,
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    // Send the request
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .headers(headers)
        .json(&body)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .context("HTTP error to Hugging Face router")?;

    let status = resp.status();
    let text_body = resp
        .text()
        .await
        .context("Failed to read Hugging Face response")?;

    if !status.is_success() {
        return Err(anyhow!("HuggingFace error {}: {}", status, text_body));
    }

    // Parse the response
    let parsed: ChatResponse = serde_json::from_str(&text_body)
        .context("Failed to parse Hugging Face JSON response")?;

    let generated = parsed
        .choices
        .first()
        .map(|choice| choice.message.content.clone())
        .ok_or_else(|| anyhow!("No choices in Hugging Face response"))?;

    Ok(generated)
}

/// Generate code with conversation history for multi-turn refinement
pub async fn generate_code_with_history(messages: Vec<Message>) -> Result<String> {
    let token = std::env::var("HF_TOKEN")
        .context("HF_TOKEN missing in .env")?;

    let url = "https://router.huggingface.co/v1/chat/completions".to_string();

    // Ensure system message is at the beginning
    let mut full_messages = vec![Message {
        role: "system".to_string(),
        content: "You are an expert Python code generator. Generate clean, well-commented, executable Python code based on user requests. \
                 Follow these rules:\n\
                 1. Output ONLY valid Python code\n\
                 2. Include helpful comments explaining the logic\n\
                 3. Use proper Python conventions and best practices\n\
                 4. Handle errors gracefully with try-except where appropriate\n\
                 5. If external libraries are needed, import them at the top\n\
                 6. Make the code production-ready and maintainable".to_string(),
    }];
    
    // Add conversation history
    full_messages.extend(messages);

    let body = ChatRequest {
        model: "Qwen/Qwen2.5-Coder-7B-Instruct".to_string(),
        messages: full_messages,
        max_tokens: Some(1024),
        temperature: Some(0.2),
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}"))
            .context("Invalid Bearer token format")?,
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .headers(headers)
        .json(&body)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .context("HTTP error to Hugging Face router")?;

    let status = resp.status();
    let text_body = resp
        .text()
        .await
        .context("Failed to read Hugging Face response")?;

    if !status.is_success() {
        return Err(anyhow!("HuggingFace error {}: {}", status, text_body));
    }

    let parsed: ChatResponse = serde_json::from_str(&text_body)
        .context("Failed to parse Hugging Face JSON response")?;

    let generated = parsed
        .choices
        .first()
        .map(|choice| choice.message.content.clone())
        .ok_or_else(|| anyhow!("No choices in Hugging Face response"))?;

    Ok(generated)
}
