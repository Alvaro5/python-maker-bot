use crate::config::AppConfig;
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
            Self::Ollama => {
                // Ollama requires no authentication by default.
                // If the user set LLM_API_KEY, honor it (some Ollama proxies use auth).
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
            Self::OpenAiCompatible => {
                // Use LLM_API_KEY if available.
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
const SYSTEM_PROMPT: &str = "You are an expert Python code generator. Generate clean, well-commented, COMPLETE and POLISHED executable Python code based on user requests. \
CRITICAL RULES:\n\
1. Output ONLY valid, executable Python code - NO markdown text, NO explanations outside comments\n\
2. DO NOT include phrases like 'Here is the code' or 'Step 1:' - these cause syntax errors\n\
3. DO NOT use markdown headings (###, ##, #) outside of Python comments\n\
4. Start directly with Python code (imports, functions, or main logic)\n\
5. Include helpful comments explaining the logic using Python's # syntax\n\
6. Use proper Python conventions and best practices\n\
7. Handle errors gracefully with try-except where appropriate\n\
8. If external libraries are needed, import them at the top\n\
9. Make the code production-ready, feature-complete, and maintainable\n\
10. The code must run immediately when executed with python3 <file>.py WITHOUT ANY ERRORS\n\
\n\
CRITICAL BUG PREVENTION:\n\
- DEFINE ALL VARIABLES before using them (e.g., if you use RED, define RED = (255, 0, 0) first)\n\
- DEFINE ALL COLOR CONSTANTS at the top (WHITE, BLACK, RED, GREEN, BLUE, YELLOW, etc.)\n\
- CHECK for empty lists before accessing indices: if len(my_list) > 0: my_list[0]\n\
- INITIALIZE all class attributes in __init__ (e.g., self.passed = False)\n\
- USE try-except for any operations that could fail\n\
- TEST all variable references - never use undefined variables\n\
\n\
FOR GAMES - CRITICAL PLAYABILITY RULES:\n\
- Games MUST be actually playable and FUN - test physics and controls!\n\
- Define ALL colors at the top: WHITE, BLACK, RED, GREEN, BLUE, YELLOW\n\
- Use VISIBLE, contrasting colors (bright colors on light/dark backgrounds)\n\
\n\
KEYBOARD CONTROLS - MUST WORK:\n\
- ALWAYS check for KEYDOWN events in the event loop\n\
- For Flappy Bird: SPACE key must make bird jump UP immediately\n\
- For Snake: Arrow keys must change direction immediately\n\
- For Pong: WASD and arrow keys must move paddles smoothly\n\
- Controls must be RESPONSIVE - player should feel in control\n\
\n\
PHYSICS - MUST FEEL GOOD:\n\
- Gravity should be reasonable (0.4 to 0.8 for Flappy Bird)\n\
- Jump/flap strength should overcome gravity easily (FLAP_STRENGTH = -8 to -12)\n\
- Movement speed should be smooth and visible\n\
- Frame rate: use 60 FPS for smooth gameplay\n\
\n\
FOR FLAPPY BIRD SPECIFICALLY:\n\
- Bird must respond to SPACE key with upward velocity\n\
- Gravity must be applied every frame: bird.velocity += GRAVITY\n\
- Flap must set velocity negative: bird.velocity = FLAP_STRENGTH (e.g., -10)\n\
- Bird position updates: bird.rect.y += int(bird.velocity)\n\
\n\
FLAPPY BIRD PIPES - CRITICAL:\n\
- Pipes ALWAYS come in PAIRS: one from top, one from bottom\n\
- There MUST be a GAP between top and bottom pipes (150-200 pixels)\n\
- Gap position should be random but not too high or too low\n\
- Example pipe creation:\n\
  gap_center = random.randint(200, 400)  # Center of gap\n\
  top_pipe_height = gap_center - GAP_SIZE//2\n\
  bottom_pipe_y = gap_center + GAP_SIZE//2\n\
- Top pipe: rect.bottom should be at (gap_center - GAP_SIZE//2)\n\
- Bottom pipe: rect.top should be at (gap_center + GAP_SIZE//2)\n\
- First pipes should spawn off-screen (x = SCREEN_WIDTH + 100)\n\
- Pipes move left at constant speed (3-5 pixels per frame)\n\
- Spawn new pipe pair every 1.5-2 seconds\n\
- Remove pipes when they go off-screen left (pipe.rect.right < 0)\n\
- Collision must be accurate but not frustrating\n\
- Score increases when bird passes the center of pipe pair\n\
- Game must be beatable - not impossible\n\
\n\
GAME STRUCTURE:\n\
- Use proper game states: 'start', 'playing', 'game_over'\n\
- Start screen with instructions (Press SPACE to start)\n\
- Display controls on screen or in comments\n\
- Game over screen with final score and restart option\n\
- Initialize ALL sprite attributes in __init__ (self.passed = False, etc.)\n\
- Check sprite groups not empty before collision checks\n\
- Proper restart: reset all variables, empty all sprite groups, recreate all sprites\n\
\n\
SELF-CONTAINED:\n\
- DO NOT load external files (sounds, images, fonts)\n\
- Use pygame.font.Font(None, size) for default fonts only\n\
- Generate all graphics with pygame.draw and Surface.fill()\n\
- Print score updates to console if sound not available\n\
\n\
TESTING:\n\
- Code must run without NameError, AttributeError, IndexError\n\
- Player must be able to play for at least 30 seconds\n\
- Controls must work on first try\n\
- Game must be FUN - not too hard, not too easy";

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
                    &text_body[..text_body.len().min(500)]
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
