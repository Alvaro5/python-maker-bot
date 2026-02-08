use crate::config::AppConfig;
use crate::utils::find_char_boundary;
use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use std::time::Duration;

// ── Provider abstraction ────────────────────────────────────────────────

/// Supported LLM providers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Provider {
    /// HuggingFace Inference API (cloud, requires HF_TOKEN).
    HuggingFace,
    /// Ollama local server (OpenAI-compatible endpoint, no auth).
    Ollama,
    /// Any OpenAI-compatible API (user-supplied URL, optional LLM_API_KEY).
    OpenAiCompatible,
}

/// Default HuggingFace API URL — used to detect whether the user explicitly
/// overrode `api_url` in the config.
const HF_DEFAULT_URL: &str = "https://router.huggingface.co/v1/chat/completions";
const OLLAMA_DEFAULT_URL: &str = "http://localhost:11434/v1/chat/completions";

impl Provider {
    /// Parse the provider string from config into a `Provider` enum.
    pub fn from_config(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "huggingface" | "hf" => Ok(Self::HuggingFace),
            "ollama" => Ok(Self::Ollama),
            "openai-compatible" | "openai" | "custom" => Ok(Self::OpenAiCompatible),
            other => Err(anyhow!(
                "Unknown provider '{}'. Supported: huggingface, ollama, openai-compatible",
                other
            )),
        }
    }

    /// Return the default API URL for this provider.
    pub fn default_api_url(&self) -> &'static str {
        match self {
            Self::HuggingFace => HF_DEFAULT_URL,
            Self::Ollama => OLLAMA_DEFAULT_URL,
            Self::OpenAiCompatible => "", // must be configured explicitly
        }
    }

    /// Human-readable name for display.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::HuggingFace => "HuggingFace",
            Self::Ollama => "Ollama (local)",
            Self::OpenAiCompatible => "OpenAI-compatible",
        }
    }

    /// Resolve the effective API URL: use the user-supplied `api_url` if it
    /// was explicitly overridden; otherwise fall back to the provider default.
    pub fn resolve_api_url(&self, configured_url: &str) -> Result<String> {
        // If the configured URL is the HF default but the provider is NOT HF,
        // the user hasn't customized it — use the provider's own default.
        if configured_url == HF_DEFAULT_URL && *self != Self::HuggingFace {
            let default = self.default_api_url();
            if default.is_empty() {
                return Err(anyhow!(
                    "Provider '{}' requires an explicit api_url in pymakebot.toml",
                    self.display_name()
                ));
            }
            return Ok(default.to_string());
        }
        Ok(configured_url.to_string())
    }

    /// Build the authorization headers for this provider.
    pub fn auth_headers(&self) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        match self {
            Self::HuggingFace => {
                let token = std::env::var("HF_TOKEN")
                    .context("HF_TOKEN missing in .env — required for HuggingFace provider")?;
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer {token}"))
                        .context("Invalid Bearer token format")?,
                );
            }
            Self::Ollama | Self::OpenAiCompatible => {
                // Ollama requires no auth by default; OpenAI-compatible may need it.
                // Honor LLM_API_KEY when set (some Ollama proxies also use auth).
                if let Ok(key) = std::env::var("LLM_API_KEY") {
                    if !key.is_empty() {
                        headers.insert(
                            AUTHORIZATION,
                            HeaderValue::from_str(&format!("Bearer {key}"))
                                .context("Invalid LLM_API_KEY format")?,
                        );
                    }
                }
            }
        }

        Ok(headers)
    }
}

// ── Request / Response types (OpenAI chat completions format) ───────────

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    /// Explicitly disable streaming (some Ollama versions default to stream).
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
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

/// System prompt used for all code-generation requests.
///
/// Design principles:
/// 1. Works across model families (Qwen, Llama, Mistral, GPT, etc.) — uses
///    plain-text role/content; the inference server handles chat templates.
/// 2. Fits comfortably in 8K-token context windows (local models).
/// 3. Front-loads the most critical constraints (output format) since models
///    weigh the start of the system message most heavily.
/// 4. Uses numbered rules and short imperative sentences for maximum
///    instruction-following across model sizes.
/// 5. Covers the two main use cases: general scripts and pygame games.
const SYSTEM_PROMPT: &str = "\
You are a Python code generator. You receive a request and you respond with a single, complete, executable Python script. Nothing else.\n\
\n\
=== OUTPUT FORMAT (MANDATORY) ===\n\
1. Respond with ONLY Python source code. No prose, no markdown headings, no \"Here is the code\".\n\
2. If you use a code fence, use exactly: ```python ... ``` with nothing outside it.\n\
3. The script must execute successfully with `python3 script.py` on the first try.\n\
4. Put all explanations inside Python # comments. Never output bare English sentences.\n\
\n\
=== CODE QUALITY ===\n\
5. Write clean, idiomatic, PEP 8-compliant Python 3.10+ code.\n\
6. Include concise docstrings for functions and classes.\n\
7. Use type hints on function signatures.\n\
8. Handle errors with try/except; never let the script crash on recoverable failures.\n\
9. Import all dependencies at the top of the file.\n\
10. Prefer the standard library when possible; use third-party packages only when they add clear value.\n\
\n\
=== BUG PREVENTION (TOP CAUSES OF FAILURE) ===\n\
11. Define every variable, constant, and class attribute BEFORE referencing it. Common miss: color tuples like RED, WHITE, BLACK.\n\
12. Initialize ALL instance attributes inside __init__.\n\
13. Guard list/dict access: check length or use .get() before indexing.\n\
14. Never use undefined names — the script must pass `py_compile` and `ruff check` with zero errors.\n\
\n\
=== PYGAME / GAME GENERATION ===\n\
When the request involves a game or graphical application:\n\
15. Define color constants at the top: WHITE = (255,255,255), BLACK = (0,0,0), etc.\n\
16. Target 60 FPS via pygame.time.Clock().tick(60).\n\
17. Handle input in the event loop (KEYDOWN/KEYUP). Controls must feel responsive.\n\
18. Implement game states: MENU → PLAYING → GAME_OVER, with restart on key press.\n\
19. Use reasonable physics: gravity 0.4–0.8 px/frame, jump impulse −8 to −12.\n\
20. Obstacles (pipes, walls, enemies) must always leave a passable gap.\n\
21. Draw everything procedurally with pygame.draw and Surface.fill — NO external image/sound/font files.\n\
22. Use pygame.font.Font(None, size) for text rendering.\n\
\n\
=== SELF-CONTAINED ===\n\
23. The script must not depend on any external files (images, JSON, CSV, audio).\n\
24. Generate or synthesize any needed data/assets at runtime.\n\
25. When audio or visual output is unavailable, fall back to console print statements.\n\
\n\
=== WHEN FIXING / REFINING CODE ===\n\
26. When asked to fix an error, output the COMPLETE corrected script — not just the changed lines.\n\
27. Preserve all existing features unless explicitly told to remove them.";

/// Generate code with conversation history for multi-turn refinement.
///
/// Routes to the configured provider (HuggingFace, Ollama, or any
/// OpenAI-compatible endpoint). All providers use the same chat
/// completions request/response format.
pub async fn generate_code_with_history(
    messages: Vec<Message>,
    config: &AppConfig,
) -> Result<String> {
    let provider = Provider::from_config(&config.provider)?;
    let api_url = provider.resolve_api_url(&config.api_url)?;
    let headers = provider.auth_headers()?;

    // Ensure system message is at the beginning
    let mut full_messages = vec![Message {
        role: "system".to_string(),
        content: SYSTEM_PROMPT.to_string(),
    }];

    // Add conversation history
    full_messages.extend(messages);

    let body = ChatRequest {
        model: config.model.clone(),
        messages: full_messages,
        max_tokens: Some(config.max_tokens),
        temperature: Some(config.temperature),
        stream: Some(false), // always disable streaming
    };

    let client = reqwest::Client::new();

    // Retry loop with exponential backoff
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..=config.max_retries {
        if attempt > 0 {
            let base_delay = Duration::from_secs(1u64 << (attempt - 1)); // 1s, 2s, 4s, ...
            let jitter = Duration::from_millis(rand::random::<u64>() % 500);
            tokio::time::sleep(base_delay + jitter).await;
        }

        let result = client
            .post(&api_url)
            .headers(headers.clone())
            .json(&body)
            .timeout(Duration::from_secs(120))
            .send()
            .await;

        let resp = match result {
            Ok(r) => r,
            Err(e) => {
                last_err = Some(anyhow!("HTTP error to {} ({}): {}", provider.display_name(), api_url, e));
                continue; // network error → retry
            }
        };

        let status = resp.status();
        let text_body = resp
            .text()
            .await
            .context("Failed to read API response")?;

        if status.is_success() {
            let parsed: ChatResponse = serde_json::from_str(&text_body)
                .with_context(|| format!(
                    "Failed to parse {} JSON response. Raw body:\n{}",
                    provider.display_name(),
                    &text_body[..find_char_boundary(&text_body, 500)]
                ))?;

            let generated = parsed
                .choices
                .first()
                .map(|choice| choice.message.content.clone())
                .ok_or_else(|| anyhow!("No choices in {} response", provider.display_name()))?;

            return Ok(generated);
        }

        // Decide whether to retry based on status code
        let code = status.as_u16();
        if code == 429 || (500..600).contains(&code) {
            last_err = Some(anyhow!("{} error {}: {}", provider.display_name(), status, text_body));
            continue; // rate-limited or server error → retry
        }

        // Client errors (400, 401, 403, etc.) — fail fast
        return Err(anyhow!("{} error {}: {}", provider.display_name(), status, text_body));
    }

    Err(last_err.unwrap_or_else(|| anyhow!("All retry attempts exhausted")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_creation() {
        let msg = Message {
            role: "user".to_string(),
            content: "test content".to_string(),
        };
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content, "test content");
    }

    #[test]
    fn test_message_clone() {
        let msg = Message {
            role: "assistant".to_string(),
            content: "response".to_string(),
        };
        let cloned = msg.clone();
        assert_eq!(msg.role, cloned.role);
        assert_eq!(msg.content, cloned.content);
    }

    #[test]
    fn test_chat_request_serialization() {
        let request = ChatRequest {
            model: "test-model".to_string(),
            messages: vec![
                Message {
                    role: "system".to_string(),
                    content: "You are helpful".to_string(),
                },
                Message {
                    role: "user".to_string(),
                    content: "Hello".to_string(),
                },
            ],
            max_tokens: Some(100),
            temperature: Some(0.5),
            stream: Some(false),
        };

        let json = serde_json::to_string(&request);
        assert!(json.is_ok());

        let json_str = json.unwrap();
        assert!(json_str.contains("test-model"));
        assert!(json_str.contains("system"));
        assert!(json_str.contains("user"));
        assert!(json_str.contains("Hello"));
    }

    #[test]
    fn test_chat_response_deserialization() {
        let json = r#"{
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": "print('Hello, World!')"
                    }
                }
            ]
        }"#;

        let response: Result<ChatResponse, _> = serde_json::from_str(json);
        assert!(response.is_ok());

        let response = response.unwrap();
        assert_eq!(response.choices.len(), 1);
        assert_eq!(response.choices[0].message.role, "assistant");
        assert!(response.choices[0].message.content.contains("print"));
    }

    #[test]
    fn test_message_vector_operations() {
        let mut messages = vec![
            Message {
                role: "user".to_string(),
                content: "First".to_string(),
            },
            Message {
                role: "assistant".to_string(),
                content: "Second".to_string(),
            },
        ];

        assert_eq!(messages.len(), 2);

        messages.push(Message {
            role: "user".to_string(),
            content: "Third".to_string(),
        });

        assert_eq!(messages.len(), 3);
        assert_eq!(messages.last().unwrap().content, "Third");
    }

    #[test]
    fn test_optional_parameters() {
        let request = ChatRequest {
            model: "test".to_string(),
            messages: vec![],
            max_tokens: None,
            temperature: None,
            stream: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        // Optional fields should not appear in JSON when None
        assert!(!json.contains("max_tokens"));
        assert!(!json.contains("temperature"));
        assert!(!json.contains("stream"));
    }

    #[test]
    fn test_system_prompt_not_empty() {
        assert!(!SYSTEM_PROMPT.is_empty());
        assert!(SYSTEM_PROMPT.contains("Python"));
    }

    // ── Provider tests ──────────────────────────────────────────────────

    #[test]
    fn test_provider_from_config_valid() {
        assert_eq!(Provider::from_config("huggingface").unwrap(), Provider::HuggingFace);
        assert_eq!(Provider::from_config("hf").unwrap(), Provider::HuggingFace);
        assert_eq!(Provider::from_config("HuggingFace").unwrap(), Provider::HuggingFace);
        assert_eq!(Provider::from_config("ollama").unwrap(), Provider::Ollama);
        assert_eq!(Provider::from_config("Ollama").unwrap(), Provider::Ollama);
        assert_eq!(Provider::from_config("openai-compatible").unwrap(), Provider::OpenAiCompatible);
        assert_eq!(Provider::from_config("openai").unwrap(), Provider::OpenAiCompatible);
        assert_eq!(Provider::from_config("custom").unwrap(), Provider::OpenAiCompatible);
    }

    #[test]
    fn test_provider_from_config_invalid() {
        assert!(Provider::from_config("unknown").is_err());
        assert!(Provider::from_config("").is_err());
    }

    #[test]
    fn test_provider_default_api_url() {
        assert_eq!(Provider::HuggingFace.default_api_url(), HF_DEFAULT_URL);
        assert_eq!(Provider::Ollama.default_api_url(), OLLAMA_DEFAULT_URL);
        assert!(Provider::OpenAiCompatible.default_api_url().is_empty());
    }

    #[test]
    fn test_provider_resolve_api_url_explicit_override() {
        // When user sets a custom URL, all providers use it
        let custom = "http://my-server:8080/v1/chat/completions";
        assert_eq!(Provider::HuggingFace.resolve_api_url(custom).unwrap(), custom);
        assert_eq!(Provider::Ollama.resolve_api_url(custom).unwrap(), custom);
        assert_eq!(Provider::OpenAiCompatible.resolve_api_url(custom).unwrap(), custom);
    }

    #[test]
    fn test_provider_resolve_api_url_auto_defaults() {
        // When api_url is the HF default but provider is Ollama → use Ollama's URL
        let resolved = Provider::Ollama.resolve_api_url(HF_DEFAULT_URL).unwrap();
        assert_eq!(resolved, OLLAMA_DEFAULT_URL);

        // HuggingFace keeps its own default
        let resolved = Provider::HuggingFace.resolve_api_url(HF_DEFAULT_URL).unwrap();
        assert_eq!(resolved, HF_DEFAULT_URL);
    }

    #[test]
    fn test_provider_resolve_api_url_openai_requires_explicit() {
        // OpenAI-compatible provider with no explicit URL → error
        assert!(Provider::OpenAiCompatible.resolve_api_url(HF_DEFAULT_URL).is_err());
    }

    #[test]
    fn test_provider_display_name() {
        assert_eq!(Provider::HuggingFace.display_name(), "HuggingFace");
        assert_eq!(Provider::Ollama.display_name(), "Ollama (local)");
        assert_eq!(Provider::OpenAiCompatible.display_name(), "OpenAI-compatible");
    }

    #[test]
    fn test_provider_ollama_auth_no_key() {
        // Ollama should not require any env var when LLM_API_KEY is unset
        std::env::remove_var("LLM_API_KEY");
        let headers = Provider::Ollama.auth_headers().unwrap();
        assert!(!headers.contains_key(AUTHORIZATION));
    }
}
