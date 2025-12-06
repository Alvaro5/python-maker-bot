use anyhow::Result;
use chrono::Local;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

pub struct Logger {
    log_file: PathBuf,
}

#[derive(Debug)]
pub struct SessionMetrics {
    pub total_requests: usize,
    pub successful_executions: usize,
    pub failed_executions: usize,
    pub api_errors: usize,
}

impl SessionMetrics {
    pub fn new() -> Self {
        Self {
            total_requests: 0,
            successful_executions: 0,
            failed_executions: 0,
            api_errors: 0,
        }
    }

    pub fn success_rate(&self) -> f64 {
        if self.total_requests == 0 {
            return 0.0;
        }
        (self.successful_executions as f64 / self.total_requests as f64) * 100.0
    }

    pub fn display(&self) {
        use colored::Colorize;
        println!("\n{}", "━━━━━━━━━ Session Statistics ━━━━━━━━━".bright_cyan().bold());
        println!("Total requests: {}", self.total_requests);
        println!("Successful executions: {}", self.successful_executions.to_string().green());
        println!("Failed executions: {}", self.failed_executions.to_string().red());
        println!("API errors: {}", self.api_errors.to_string().yellow());
        println!("Success rate: {:.1}%", self.success_rate());
        println!("{}", "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_cyan());
    }
}

impl Logger {
    pub fn new(log_dir: &str) -> Result<Self> {
        let dir = PathBuf::from(log_dir);
        if !dir.exists() {
            fs::create_dir_all(&dir)?;
        }

        let timestamp = Local::now().format("%Y%m%d_%H%M%S");
        let log_file = dir.join(format!("session_{}.log", timestamp));

        Ok(Self { log_file })
    }

    pub fn log(&self, message: &str) -> Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_file)?;

        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
        writeln!(file, "[{}] {}", timestamp, message)?;
        Ok(())
    }

    pub fn log_api_request(&self, prompt: &str) -> Result<()> {
        self.log(&format!("API REQUEST: {}", prompt))
    }

    pub fn log_api_response(&self, response: &str) -> Result<()> {
        let preview = if response.len() > 200 {
            format!("{}...", &response[..200])
        } else {
            response.to_string()
        };
        self.log(&format!("API RESPONSE: {}", preview))
    }

    pub fn log_execution(&self, success: bool, output: &str) -> Result<()> {
        let status = if success { "SUCCESS" } else { "FAILED" };
        self.log(&format!("EXECUTION {}: {}", status, output))
    }

    pub fn log_error(&self, error: &str) -> Result<()> {
        self.log(&format!("ERROR: {}", error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_session_metrics_new() {
        let metrics = SessionMetrics::new();
        assert_eq!(metrics.total_requests, 0);
        assert_eq!(metrics.successful_executions, 0);
        assert_eq!(metrics.failed_executions, 0);
        assert_eq!(metrics.api_errors, 0);
    }

    #[test]
    fn test_success_rate_zero_requests() {
        let metrics = SessionMetrics::new();
        assert_eq!(metrics.success_rate(), 0.0);
    }

    #[test]
    fn test_success_rate_calculation() {
        let mut metrics = SessionMetrics::new();
        metrics.total_requests = 10;
        metrics.successful_executions = 8;
        assert_eq!(metrics.success_rate(), 80.0);
    }

    #[test]
    fn test_success_rate_perfect() {
        let mut metrics = SessionMetrics::new();
        metrics.total_requests = 5;
        metrics.successful_executions = 5;
        assert_eq!(metrics.success_rate(), 100.0);
    }

    #[test]
    fn test_logger_creation() {
        let test_log_dir = "test_logs_temp";
        let logger = Logger::new(test_log_dir);
        assert!(logger.is_ok());
        
        let logger = logger.unwrap();
        // Check that the parent directory exists
        assert!(logger.log_file.parent().unwrap().exists());
        
        // Clean up
        let _ = fs::remove_dir_all(test_log_dir);
    }

    #[test]
    fn test_logger_basic_log() {
        let test_log_dir = "test_logs_temp2";
        let logger = Logger::new(test_log_dir).unwrap();
        
        let result = logger.log("Test message");
        assert!(result.is_ok());
        
        // Verify log file has content
        let content = fs::read_to_string(&logger.log_file).unwrap();
        assert!(content.contains("Test message"));
        
        // Clean up
        let _ = fs::remove_dir_all(test_log_dir);
    }

    #[test]
    fn test_logger_api_request() {
        let test_log_dir = "test_logs_temp3";
        let logger = Logger::new(test_log_dir).unwrap();
        
        let result = logger.log_api_request("Create a hello world script");
        assert!(result.is_ok());
        
        let content = fs::read_to_string(&logger.log_file).unwrap();
        assert!(content.contains("API REQUEST"));
        assert!(content.contains("hello world"));
        
        // Clean up
        let _ = fs::remove_dir_all(test_log_dir);
    }

    #[test]
    fn test_logger_multiple_entries() {
        let test_log_dir = "test_logs_temp4";
        let logger = Logger::new(test_log_dir).unwrap();
        
        let _ = logger.log("Entry 1");
        let _ = logger.log("Entry 2");
        let _ = logger.log("Entry 3");
        
        let content = fs::read_to_string(&logger.log_file).unwrap();
        assert!(content.contains("Entry 1"));
        assert!(content.contains("Entry 2"));
        assert!(content.contains("Entry 3"));
        
        // Clean up
        let _ = fs::remove_dir_all(test_log_dir);
    }
}
