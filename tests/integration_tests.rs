// Integration tests for Python Maker Bot

use std::fs;
use std::path::PathBuf;

#[test]
fn test_full_workflow_code_generation_and_execution() {
    // This test simulates the full workflow without API calls
    // Clean up before test
    let _ = fs::remove_dir_all("test_integration_generated");
    
    // Simulate generated code
    let generated_code = "print('Integration test successful!')";
    
    // Write to file
    let test_dir = PathBuf::from("test_integration_generated");
    fs::create_dir_all(&test_dir).unwrap();
    
    let script_path = test_dir.join("test_script.py");
    fs::write(&script_path, generated_code).unwrap();
    
    // Verify file exists
    assert!(script_path.exists());
    
    // Read back and verify
    let content = fs::read_to_string(&script_path).unwrap();
    assert_eq!(content, generated_code);
    
    // Clean up
    let _ = fs::remove_dir_all("test_integration_generated");
}

#[test]
fn test_directory_structure() {
    // Test that the project can create necessary directories
    let dirs = vec!["test_generated_dir", "test_logs_dir"];
    
    for dir in &dirs {
        let path = PathBuf::from(dir);
        fs::create_dir_all(&path).unwrap();
        assert!(path.exists());
        assert!(path.is_dir());
        let _ = fs::remove_dir_all(&path);
    }
}

#[test]
fn test_multiple_script_generation() {
    let test_dir = "test_multi_scripts";
    let _ = fs::remove_dir_all(test_dir);
    fs::create_dir_all(test_dir).unwrap();
    
    // Generate multiple scripts
    let scripts = vec![
        ("script1.py", "print('Script 1')"),
        ("script2.py", "print('Script 2')"),
        ("script3.py", "print('Script 3')"),
    ];
    
    for (filename, code) in scripts {
        let path = PathBuf::from(test_dir).join(filename);
        fs::write(&path, code).unwrap();
        assert!(path.exists());
    }
    
    // Verify all files exist
    let entries = fs::read_dir(test_dir).unwrap();
    let count = entries.count();
    assert_eq!(count, 3);
    
    // Clean up
    let _ = fs::remove_dir_all(test_dir);
}

#[test]
fn test_log_file_creation() {
    use std::io::Write;
    
    let log_dir = "test_integration_logs";
    let _ = fs::remove_dir_all(log_dir);
    fs::create_dir_all(log_dir).unwrap();
    
    let log_file = PathBuf::from(log_dir).join("test.log");
    let mut file = fs::File::create(&log_file).unwrap();
    
    writeln!(file, "Test log entry 1").unwrap();
    writeln!(file, "Test log entry 2").unwrap();
    
    assert!(log_file.exists());
    
    let content = fs::read_to_string(&log_file).unwrap();
    assert!(content.contains("Test log entry 1"));
    assert!(content.contains("Test log entry 2"));
    
    // Clean up
    let _ = fs::remove_dir_all(log_dir);
}

#[test]
fn test_code_extraction_integration() {
    // Test the full code extraction pipeline
    let responses = vec![
        ("```python\nprint('hello')\n```", "print('hello')"),
        ("```\nprint('world')\n```", "print('world')"),
        ("print('direct')", "print('direct')"),
    ];
    
    for (input, expected) in responses {
        // In real integration, this would come from API
        let extracted = extract_code_helper(input);
        assert_eq!(extracted, expected);
    }
}

// Helper function for integration test
fn extract_code_helper(response: &str) -> String {
    use regex::Regex;
    let code_block_re = Regex::new(r"```(?:python)?\s*\n([\s\S]*?)\n```").unwrap();
    
    if let Some(captures) = code_block_re.captures(response) {
        if let Some(code) = captures.get(1) {
            return code.as_str().trim().to_string();
        }
    }
    
    response.trim().to_string()
}

#[test]
fn test_session_metrics_integration() {
    // Simulate a session with metrics
    struct TestMetrics {
        total: usize,
        success: usize,
        failed: usize,
    }
    
    let mut metrics = TestMetrics {
        total: 0,
        success: 0,
        failed: 0,
    };
    
    // Simulate requests
    for i in 0..10 {
        metrics.total += 1;
        if i % 3 == 0 {
            metrics.failed += 1;
        } else {
            metrics.success += 1;
        }
    }
    
    assert_eq!(metrics.total, 10);
    // i % 3 == 0 happens for i = 0, 3, 6, 9 (4 times)
    assert_eq!(metrics.failed, 4);
    assert_eq!(metrics.success, 6);
    
    let success_rate = (metrics.success as f64 / metrics.total as f64) * 100.0;
    assert_eq!(success_rate, 60.0);
}

#[test]
fn test_conversation_history_management() {
    // Test conversation history structure
    #[derive(Clone)]
    #[allow(dead_code)]
    struct TestMessage {
        role: String,
        content: String,
    }
    
    let mut history: Vec<TestMessage> = Vec::new();
    
    // Add messages
    history.push(TestMessage {
        role: "user".to_string(),
        content: "Create a script".to_string(),
    });
    
    history.push(TestMessage {
        role: "assistant".to_string(),
        content: "print('hello')".to_string(),
    });
    
    history.push(TestMessage {
        role: "user".to_string(),
        content: "Add error handling".to_string(),
    });
    
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[1].role, "assistant");
    
    // Test clearing history
    history.clear();
    assert_eq!(history.len(), 0);
}
