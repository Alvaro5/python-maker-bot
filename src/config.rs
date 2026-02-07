use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

/// Application configuration, loaded from `.pymakebot.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub provider: String,
    pub model: String,
    pub api_url: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub execution_timeout_secs: u64,
    pub auto_install_deps: bool,
    pub max_history_messages: usize,
    pub max_retries: u32,
    pub use_docker: bool,
    pub use_venv: bool,
    pub log_dir: String,
    pub generated_dir: String,
    pub python_executable: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            provider: "huggingface".to_string(),
            model: "Qwen/Qwen2.5-Coder-32B-Instruct".to_string(),
            api_url: "https://router.huggingface.co/v1/chat/completions".to_string(),
            max_tokens: 16284,
            temperature: 0.2,
            execution_timeout_secs: 30,
            auto_install_deps: false,
            max_history_messages: 20,
            max_retries: 3,
            use_docker: false,
            use_venv: true,
            log_dir: "logs".to_string(),
            generated_dir: "generated".to_string(),
            python_executable: "python3".to_string(),
        }
    }
}

impl AppConfig {
    /// Load configuration with the chain: `./pymakebot.toml` -> `~/.pymakebot.toml` -> defaults.
    pub fn load() -> Self {
        let candidates = Self::config_paths();
        for path in &candidates {
            if let Ok(contents) = fs::read_to_string(path) {
                match toml::from_str::<AppConfig>(&contents) {
                    Ok(cfg) => return cfg,
                    Err(e) => {
                        eprintln!("Warning: failed to parse {}: {}", path.display(), e);
                    }
                }
            }
        }
        Self::default()
    }

    fn config_paths() -> Vec<PathBuf> {
        let mut paths = vec![PathBuf::from("pymakebot.toml")];
        if let Some(home) = dirs::home_dir() {
            paths.push(home.join("pymakebot.toml"));
        }
        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.provider, "huggingface");
        assert_eq!(cfg.model, "Qwen/Qwen2.5-Coder-32B-Instruct");
        assert_eq!(cfg.max_tokens, 16284);
        assert_eq!(cfg.temperature, 0.2);
        assert_eq!(cfg.execution_timeout_secs, 30);
        assert!(!cfg.auto_install_deps);
        assert_eq!(cfg.max_history_messages, 20);
        assert_eq!(cfg.max_retries, 3);
        assert!(!cfg.use_docker);
        assert!(cfg.use_venv);
        assert_eq!(cfg.log_dir, "logs");
        assert_eq!(cfg.python_executable, "python3");
        assert_eq!(cfg.generated_dir, "generated");
    }

    #[test]
    fn test_partial_toml_deserialize() {
        let toml_str = r#"
            model = "custom-model"
            max_tokens = 8000
        "#;
        let cfg: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.model, "custom-model");
        assert_eq!(cfg.max_tokens, 8000);
        // Other fields should be defaults
        assert_eq!(cfg.temperature, 0.2);
        assert_eq!(cfg.max_retries, 3);
    }

    #[test]
    fn test_full_toml_deserialize() {
        let toml_str = r#"
            model = "test-model"
            api_url = "https://example.com/v1/chat"
            max_tokens = 4096
            temperature = 0.5
            execution_timeout_secs = 60
            auto_install_deps = true
            max_history_messages = 10
            max_retries = 5
            use_docker = true
            log_dir = "my_logs"
            generated_dir = "my_scripts"
            python_executable = "python"
        "#;
        let cfg: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.model, "test-model");
        assert_eq!(cfg.api_url, "https://example.com/v1/chat");
        assert_eq!(cfg.max_tokens, 4096);
        assert_eq!(cfg.temperature, 0.5);
        assert_eq!(cfg.execution_timeout_secs, 60);
        assert!(cfg.auto_install_deps);
        assert_eq!(cfg.max_history_messages, 10);
        assert_eq!(cfg.max_retries, 5);
        assert!(cfg.use_docker);
        assert_eq!(cfg.log_dir, "my_logs");
        assert_eq!(cfg.generated_dir, "my_scripts");
    }

    #[test]
    fn test_load_falls_back_to_defaults() {
        // When no config file exists, load() returns defaults
        let cfg = AppConfig::load();
        assert_eq!(cfg.max_retries, AppConfig::default().max_retries);
    }
}
