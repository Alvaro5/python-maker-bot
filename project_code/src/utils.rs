use anyhow::{Context, Result};
use regex::Regex;
use std::fs;
use std::path::Path;

pub fn ensure_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::create_dir_all(path)
            .with_context(|| format!("Impossible de créer le dossier {:?}", path))?;
    }
    Ok(())
}

/// Extract Python code from a response that might contain markdown code blocks
/// Handles formats like:
/// - ```python\ncode\n```
/// - ```\ncode\n```
/// - plain code without markers
pub fn extract_python_code(response: &str) -> String {
    // Try to match markdown code blocks with optional language identifier
    let code_block_re = Regex::new(r"```(?:python)?\s*\n([\s\S]*?)\n```").unwrap();
    
    if let Some(captures) = code_block_re.captures(response) {
        if let Some(code) = captures.get(1) {
            return code.as_str().trim().to_string();
        }
    }
    
    // If no markdown block found, return trimmed response as-is
    response.trim().to_string()
}
