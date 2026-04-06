use anyhow::{anyhow, Result};
use std::path::Path;

use crate::config::AppConfig;

/// In-memory RAG store: pairs of (chunk_text, embedding_vector).
pub struct RagStore {
    entries: Vec<(String, Vec<f32>)>,
}

impl Default for RagStore {
    fn default() -> Self {
        Self::new()
    }
}

impl RagStore {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Returns true if the store contains no chunks.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Load a file, chunk it, embed each chunk, and add to the store.
    pub async fn load_file(&mut self, path: &str, config: &AppConfig) -> Result<usize> {
        let text = read_file_as_text(path)?;
        let chunks = chunk_text(&text, config.rag_chunk_size);
        let count = chunks.len();

        for chunk in &chunks {
            let embedding =
                crate::api::generate_embeddings(chunk, &config.rag_embedding_model, config).await?;
            self.entries.push((chunk.clone(), embedding));
        }

        Ok(count)
    }

    /// Embed a query and return the top-n most similar chunks.
    pub async fn retrieve(&self, query: &str, n: usize, config: &AppConfig) -> Result<Vec<String>> {
        if self.entries.is_empty() {
            return Ok(Vec::new());
        }

        let query_embedding =
            crate::api::generate_embeddings(query, &config.rag_embedding_model, config).await?;

        let mut scored: Vec<(f32, &str)> = self
            .entries
            .iter()
            .map(|(text, emb)| (cosine_similarity(&query_embedding, emb), text.as_str()))
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        Ok(scored
            .into_iter()
            .take(n)
            .map(|(_, t)| t.to_string())
            .collect())
    }
}

/// Read a file and convert its contents to plain text.
/// Supports TXT, MD (read directly) and CSV (converted to "header: value" format).
pub fn read_file_as_text(path: &str) -> Result<String> {
    let path = Path::new(path);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    // Validate format before attempting to read
    if !matches!(ext.as_str(), "csv" | "txt" | "md") {
        return Err(anyhow!(
            "Unsupported file format '.{}'. Supported: txt, md, csv",
            ext
        ));
    }

    let raw = std::fs::read_to_string(path)
        .map_err(|e| anyhow!("Failed to read '{}': {}", path.display(), e))?;

    match ext.as_str() {
        "csv" => csv_to_text(&raw),
        _ => Ok(raw),
    }
}

/// Convert CSV content into "header: value" text per row.
fn csv_to_text(csv_content: &str) -> Result<String> {
    let mut lines = csv_content.lines();
    let header_line = lines.next().ok_or_else(|| anyhow!("CSV file is empty"))?;
    let headers: Vec<&str> = header_line.split(',').map(|h| h.trim()).collect();

    let mut output = String::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let values: Vec<&str> = line.split(',').map(|v| v.trim()).collect();
        for (i, header) in headers.iter().enumerate() {
            let value = values.get(i).unwrap_or(&"");
            output.push_str(&format!("{}: {}\n", header, value));
        }
        output.push('\n');
    }

    Ok(output)
}

/// Split text into chunks of approximately `chunk_size` characters with 10% overlap.
/// Prefers splitting at paragraph or line boundaries.
pub fn chunk_text(text: &str, chunk_size: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }

    if text.len() <= chunk_size {
        return vec![text.to_string()];
    }

    let overlap = chunk_size / 10;
    let mut chunks = Vec::new();
    let mut start = 0;

    while start < text.len() {
        let end = (start + chunk_size).min(text.len());
        let slice = &text[start..end];

        // Try to break at a paragraph boundary (double newline), then single newline
        let actual_end = if end < text.len() {
            if let Some(pos) = slice.rfind("\n\n") {
                start + pos + 2
            } else if let Some(pos) = slice.rfind('\n') {
                start + pos + 1
            } else {
                end
            }
        } else {
            end
        };

        chunks.push(text[start..actual_end].to_string());

        // Advance with overlap
        let advance = actual_end.saturating_sub(start).saturating_sub(overlap);
        if advance == 0 {
            // Avoid infinite loop if chunk_size is very small
            start = actual_end;
        } else {
            start += advance;
        }
    }

    chunks
}

/// Compute cosine similarity between two vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot / (norm_a * norm_b)
}

/// Format retrieved chunks and user prompt into an augmented RAG prompt.
pub fn build_rag_prompt(chunks: &[String], user_prompt: &str) -> String {
    let context = chunks.join("\n---\n");
    format!(
        "Context:\n{}\n\nUse this context to answer:\n{}",
        context, user_prompt
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_text_short() {
        let text = "Hello world";
        let chunks = chunk_text(text, 1024);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Hello world");
    }

    #[test]
    fn test_chunk_text_empty() {
        let chunks = chunk_text("", 1024);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_text_splits_at_boundaries() {
        let text = "Paragraph one.\n\nParagraph two.\n\nParagraph three.\n\nParagraph four.";
        let chunks = chunk_text(text, 30);
        assert!(chunks.len() > 1);
        // Each chunk should end at a paragraph or line boundary
        for chunk in &chunks {
            assert!(!chunk.is_empty());
        }
    }

    #[test]
    fn test_chunk_text_overlap() {
        // With overlap, the last part of chunk N should appear at the start of chunk N+1
        let text = "A\n\nB\n\nC\n\nD\n\nE\n\nF\n\nG\n\nH\n\nI\n\nJ";
        let chunks = chunk_text(text, 10);
        assert!(chunks.len() > 1);
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_empty() {
        let sim = cosine_similarity(&[], &[]);
        assert_eq!(sim, 0.0);
    }

    #[test]
    fn test_cosine_similarity_mismatched() {
        let a = vec![1.0, 2.0];
        let b = vec![1.0];
        let sim = cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0);
    }

    #[test]
    fn test_csv_to_text() {
        let csv = "name,age,city\nAlice,30,NYC\nBob,25,LA\n";
        let result = csv_to_text(csv).unwrap();
        assert!(result.contains("name: Alice"));
        assert!(result.contains("age: 30"));
        assert!(result.contains("city: NYC"));
        assert!(result.contains("name: Bob"));
        assert!(result.contains("age: 25"));
        assert!(result.contains("city: LA"));
    }

    #[test]
    fn test_csv_to_text_empty() {
        assert!(csv_to_text("").is_err());
    }

    #[test]
    fn test_build_rag_prompt() {
        let chunks = vec![
            "chunk one content".to_string(),
            "chunk two content".to_string(),
        ];
        let result = build_rag_prompt(&chunks, "What is this about?");
        assert!(result.starts_with("Context:\n"));
        assert!(result.contains("chunk one content"));
        assert!(result.contains("---"));
        assert!(result.contains("chunk two content"));
        assert!(result.contains("Use this context to answer:"));
        assert!(result.contains("What is this about?"));
    }

    #[test]
    fn test_build_rag_prompt_empty_chunks() {
        let result = build_rag_prompt(&[], "My question");
        assert!(result.contains("Use this context to answer:\nMy question"));
    }

    #[test]
    fn test_read_file_unsupported_format() {
        let result = read_file_as_text("test.pdf");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unsupported file format"));
    }

    #[test]
    fn test_rag_store_new_is_empty() {
        let store = RagStore::new();
        assert!(store.is_empty());
    }
}
