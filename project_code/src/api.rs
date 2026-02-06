use crate::config::AppConfig;
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

/// Generate code with conversation history for multi-turn refinement
pub async fn generate_code_with_history(
    messages: Vec<Message>,
    config: &AppConfig,
) -> Result<String> {
    let token = std::env::var("HF_TOKEN")
        .context("HF_TOKEN missing in .env")?;

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
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}"))
            .context("Invalid Bearer token format")?,
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

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
            .post(&config.api_url)
            .headers(headers.clone())
            .json(&body)
            .timeout(Duration::from_secs(60))
            .send()
            .await;

        let resp = match result {
            Ok(r) => r,
            Err(e) => {
                last_err = Some(anyhow!("HTTP error to Hugging Face router: {}", e));
                continue; // network error → retry
            }
        };

        let status = resp.status();
        let text_body = resp
            .text()
            .await
            .context("Failed to read Hugging Face response")?;

        if status.is_success() {
            let parsed: ChatResponse = serde_json::from_str(&text_body)
                .context("Failed to parse Hugging Face JSON response")?;

            let generated = parsed
                .choices
                .first()
                .map(|choice| choice.message.content.clone())
                .ok_or_else(|| anyhow!("No choices in Hugging Face response"))?;

            return Ok(generated);
        }

        // Decide whether to retry based on status code
        let code = status.as_u16();
        if code == 429 || (500..600).contains(&code) {
            last_err = Some(anyhow!("HuggingFace error {}: {}", status, text_body));
            continue; // rate-limited or server error → retry
        }

        // Client errors (400, 401, 403, etc.) — fail fast
        return Err(anyhow!("HuggingFace error {}: {}", status, text_body));
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
        };

        let json = serde_json::to_string(&request).unwrap();
        // Optional fields should not appear in JSON when None
        assert!(!json.contains("max_tokens"));
        assert!(!json.contains("temperature"));
    }

    #[test]
    fn test_system_prompt_not_empty() {
        assert!(!SYSTEM_PROMPT.is_empty());
        assert!(SYSTEM_PROMPT.contains("Python"));
    }
}
