use crate::utils::{ensure_dir, extract_imports, is_stdlib};
use anyhow::{Context, Result};
use chrono::Utc;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

/// Mode d'exécution pour les scripts Python
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExecutionMode {
    /// Mode par défaut: capture stdout/stderr
    Captured,
    /// Mode interactif: hérite stdio (pour jeux, input utilisateur)
    Interactive,
}

/// Résultat de l'exécution d'un script Python.
pub struct CodeExecutionResult {
    pub script_path: PathBuf,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

impl CodeExecutionResult {
    /// Returns true only when the process exited with code 0.
    pub fn is_success(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// Responsable de l'écriture des scripts Python sur le disque et de leur exécution.
pub struct CodeExecutor {
    base_dir: PathBuf,
}

impl CodeExecutor {
    /// Crée un exécuteur de code.
    ///
    /// `base_dir` : répertoire où seront stockés les scripts générés.
    pub fn new(base_dir: &str) -> Result<Self> {
        let dir = PathBuf::from(base_dir);
        ensure_dir(&dir)?;
        Ok(Self { base_dir: dir })
    }

    /// Detect non-standard library dependencies in Python code
    pub fn detect_dependencies(&self, code: &str) -> Vec<String> {
        let all_imports = extract_imports(code);
        all_imports
            .into_iter()
            .filter(|pkg| !is_stdlib(pkg))
            .collect()
    }

    /// Install Python packages using pip
    pub fn install_packages(&self, packages: &[String]) -> Result<()> {
        if packages.is_empty() {
            return Ok(());
        }

        println!("Installing dependencies: {}", packages.join(", "));

        let python_cmds = ["python3", "python"];
        let mut last_err: Option<anyhow::Error> = None;

        for cmd in python_cmds {
            let mut args = vec!["-m", "pip", "install", "--quiet"];
            args.extend(packages.iter().map(|s| s.as_str()));

            let output = Command::new(cmd).args(&args).output();

            match output {
                Ok(out) => {
                    if out.status.success() {
                        println!("✓ Dependencies installed successfully");
                        return Ok(());
                    } else {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        last_err = Some(anyhow::anyhow!(
                            "pip install failed: {}",
                            stderr
                        ));
                    }
                }
                Err(e) => {
                    last_err = Some(anyhow::anyhow!(
                        "Failed to run pip with {}: {}",
                        cmd,
                        e
                    ));
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            anyhow::anyhow!("Could not install packages with python/python3")
        }))
    }

    /// Détecte si le code nécessite une exécution interactive (pygame, input(), etc.)
    pub fn needs_interactive_mode(&self, code: &str) -> bool {
        let interactive_keywords = [
            "pygame",
            "input(",
            "turtle",
            "tkinter",
            "curses",
            "getpass",
            "cv2.imshow",
            "plt.show",
            "matplotlib",
        ];

        interactive_keywords.iter().any(|keyword| code.contains(keyword))
    }

    /// Write a Python script to disk, returning the path.
    pub fn write_script(&self, code: &str) -> Result<PathBuf> {
        let ts = Utc::now().format("%Y%m%d_%H%M%S");
        let filename = format!("script_{ts}.py");
        let script_path = self.base_dir.join(filename);
        fs::write(&script_path, code)
            .with_context(|| format!("Could not write the script {:?}", script_path))?;
        Ok(script_path)
    }

    /// Run `python3 -m py_compile <path>` and return Ok(()) on success or
    /// Err(message) with the compiler output on failure.
    pub fn syntax_check(&self, path: &PathBuf) -> Result<(), String> {
        let python_cmds = ["python3", "python"];
        for cmd in python_cmds {
            let output = Command::new(cmd)
                .args(["-m", "py_compile"])
                .arg(path)
                .output();

            match output {
                Ok(out) => {
                    if out.status.success() {
                        return Ok(());
                    } else {
                        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                        return Err(stderr);
                    }
                }
                Err(_) => continue, // try next interpreter
            }
        }
        Err("Could not run syntax check with python/python3".to_string())
    }

    /// Écrit un script Python dans un fichier et l'exécute avec l'interpréteur `python` ou `python3`.
    ///
    /// Attention : ce code exécute du Python généré automatiquement.
    /// À n'utiliser que dans un environnement de test contrôlé.
    #[allow(dead_code)] // Used by tests
    pub fn write_and_run(&self, code: &str) -> Result<CodeExecutionResult> {
        self.write_and_run_with_mode(code, ExecutionMode::Captured)
    }

    /// Écrit et exécute un script Python avec le mode d'exécution spécifié.
    pub fn write_and_run_with_mode(&self, code: &str, mode: ExecutionMode) -> Result<CodeExecutionResult> {
        let script_path = self.write_script(code)?;
        self.execute_script(&script_path, mode, 0) // 0 = no timeout
    }

    /// Exécute un script Python existant avec le mode d'exécution spécifié.
    pub fn run_existing_script(&self, script_path: &str, mode: ExecutionMode, timeout_secs: u64) -> Result<CodeExecutionResult> {
        let path = PathBuf::from(script_path);
        if !path.exists() {
            return Err(anyhow::anyhow!("Script not found: {}", script_path));
        }
        self.execute_script(&path, mode, timeout_secs)
    }

    /// Execute a Python script. `timeout_secs == 0` means no timeout.
    /// Timeout only applies to `Captured` mode.
    pub fn execute_script(&self, script_path: &PathBuf, mode: ExecutionMode, timeout_secs: u64) -> Result<CodeExecutionResult> {
        // On essaie d'abord `python3`, puis `python` si besoin.
        let python_cmds = ["python3", "python"];

        let mut last_err: Option<anyhow::Error> = None;

        for cmd in python_cmds {
            match mode {
                ExecutionMode::Interactive => {
                    // Mode interactif: hérite stdin/stdout/stderr pour l'interaction utilisateur
                    // No timeout for interactive mode
                    let child = Command::new(cmd)
                        .arg(script_path)
                        .stdin(Stdio::inherit())
                        .stdout(Stdio::inherit())
                        .stderr(Stdio::inherit())
                        .spawn();

                    match child {
                        Ok(mut process) => {
                            let status = process.wait()
                                .with_context(|| format!("Failed to wait for process with {}", cmd))?;

                            return Ok(CodeExecutionResult {
                                script_path: script_path.clone(),
                                stdout: String::from("[Interactive mode - output displayed directly]"),
                                stderr: String::new(),
                                exit_code: status.code(),
                            });
                        }
                        Err(e) => {
                            last_err = Some(anyhow::anyhow!(
                                "Failed to spawn interactive process with `{cmd}`: {e}"
                            ));
                        }
                    }
                }
                ExecutionMode::Captured => {
                    // Mode capturé: spawn + optional timeout
                    let child = Command::new(cmd)
                        .arg(script_path)
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn();

                    match child {
                        Ok(mut process) => {
                            if timeout_secs > 0 {
                                let timeout = Duration::from_secs(timeout_secs);
                                match process.wait_timeout(timeout)
                                    .with_context(|| format!("Failed to wait for process with {}", cmd))?
                                {
                                    Some(status) => {
                                        let stdout = read_pipe(process.stdout.take());
                                        let stderr = read_pipe(process.stderr.take());
                                        return Ok(CodeExecutionResult {
                                            script_path: script_path.clone(),
                                            stdout,
                                            stderr,
                                            exit_code: status.code(),
                                        });
                                    }
                                    None => {
                                        // Timed out — kill the process
                                        let _ = process.kill();
                                        let _ = process.wait();
                                        return Ok(CodeExecutionResult {
                                            script_path: script_path.clone(),
                                            stdout: String::new(),
                                            stderr: format!(
                                                "Process timed out after {} seconds. \
                                                 You can increase this with execution_timeout_secs in pymakebot.toml",
                                                timeout_secs
                                            ),
                                            exit_code: None,
                                        });
                                    }
                                }
                            } else {
                                // No timeout — blocking wait
                                let output = process.wait_with_output()
                                    .with_context(|| format!("Failed to wait for process with {}", cmd))?;
                                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                                return Ok(CodeExecutionResult {
                                    script_path: script_path.clone(),
                                    stdout,
                                    stderr,
                                    exit_code: output.status.code(),
                                });
                            }
                        }
                        Err(e) => {
                            last_err = Some(anyhow::anyhow!(
                                "Failed with command `{cmd}`: {e}"
                            ));
                        }
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!(
            "Could not execute the script with python/python3"
        )))
    }
}

/// Helper to read a piped child stdio handle into a String.
fn read_pipe<R: std::io::Read>(pipe: Option<R>) -> String {
    match pipe {
        Some(mut r) => {
            let mut buf = Vec::new();
            let _ = std::io::Read::read_to_end(&mut r, &mut buf);
            String::from_utf8_lossy(&buf).to_string()
        }
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_executor_creation() {
        let temp_dir = "test_executor_temp";
        let executor = CodeExecutor::new(temp_dir);
        assert!(executor.is_ok());

        // Clean up
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_detect_dependencies_stdlib_only() {
        let executor = CodeExecutor::new("test_temp").unwrap();
        let code = "import os\nimport sys\nfrom pathlib import Path";
        let deps = executor.detect_dependencies(code);
        assert!(deps.is_empty());
        let _ = fs::remove_dir_all("test_temp");
    }

    #[test]
    fn test_detect_dependencies_third_party() {
        let executor = CodeExecutor::new("test_temp").unwrap();
        let code = "import numpy\nfrom pandas import DataFrame\nimport requests";
        let deps = executor.detect_dependencies(code);
        assert_eq!(deps.len(), 3);
        assert!(deps.contains(&"numpy".to_string()));
        assert!(deps.contains(&"pandas".to_string()));
        assert!(deps.contains(&"requests".to_string()));
        let _ = fs::remove_dir_all("test_temp");
    }

    #[test]
    fn test_detect_dependencies_mixed() {
        let executor = CodeExecutor::new("test_temp").unwrap();
        let code = "import os\nimport numpy\nimport sys\nfrom flask import Flask";
        let deps = executor.detect_dependencies(code);
        assert_eq!(deps.len(), 2);
        assert!(deps.contains(&"numpy".to_string()));
        assert!(deps.contains(&"flask".to_string()));
        let _ = fs::remove_dir_all("test_temp");
    }

    #[test]
    fn test_write_and_run_simple_script() {
        let executor = CodeExecutor::new("test_generated_simple").unwrap();
        let code = "print('Hello, Test!')";

        let result = executor.write_and_run(code);
        assert!(result.is_ok());

        let output = result.unwrap();
        // Check that script was created and executed
        let script_exists = output.script_path.exists();
        // Check that either stdout or stderr is not empty (script executed)
        assert!(!output.stdout.is_empty() || !output.stderr.is_empty());
        assert!(script_exists);

        // Clean up
        let _ = fs::remove_dir_all("test_generated_simple");
    }

    #[test]
    fn test_write_and_run_with_calculation() {
        let executor = CodeExecutor::new("test_generated_calc").unwrap();
        let code = "result = 2 + 2\nprint(f'Result: {result}')";

        let result = executor.write_and_run(code);
        assert!(result.is_ok());

        let output = result.unwrap();
        // Check execution happened (either output or error exists)
        assert!(!output.stdout.is_empty() || !output.stderr.is_empty());

        // Clean up
        let _ = fs::remove_dir_all("test_generated_calc");
    }

    #[test]
    fn test_write_and_run_error_script() {
        let executor = CodeExecutor::new("test_generated_error").unwrap();
        let code = "print(undefined_variable)";

        let result = executor.write_and_run(code);
        assert!(result.is_ok()); // Execution succeeds even with errors

        let output = result.unwrap();
        // Script was created
        let script_exists = output.script_path.exists();
        assert!(script_exists);

        // Clean up
        let _ = fs::remove_dir_all("test_generated_error");
    }

    #[test]
    fn test_install_packages_empty_list() {
        let executor = CodeExecutor::new("test_temp").unwrap();
        let result = executor.install_packages(&[]);
        assert!(result.is_ok());
        let _ = fs::remove_dir_all("test_temp");
    }

    #[test]
    fn test_needs_interactive_mode_pygame() {
        let executor = CodeExecutor::new("test_temp").unwrap();
        let code = "import pygame\npygame.init()";
        assert!(executor.needs_interactive_mode(code));
        let _ = fs::remove_dir_all("test_temp");
    }

    #[test]
    fn test_needs_interactive_mode_input() {
        let executor = CodeExecutor::new("test_temp").unwrap();
        let code = "name = input('Enter your name: ')";
        assert!(executor.needs_interactive_mode(code));
        let _ = fs::remove_dir_all("test_temp");
    }

    #[test]
    fn test_needs_interactive_mode_simple_script() {
        let executor = CodeExecutor::new("test_temp").unwrap();
        let code = "print('Hello, World!')";
        assert!(!executor.needs_interactive_mode(code));
        let _ = fs::remove_dir_all("test_temp");
    }

    #[test]
    fn test_needs_interactive_mode_matplotlib() {
        let executor = CodeExecutor::new("test_temp").unwrap();
        let code = "import matplotlib.pyplot as plt\nplt.show()";
        assert!(executor.needs_interactive_mode(code));
        let _ = fs::remove_dir_all("test_temp");
    }

    #[test]
    fn test_execution_mode_enum() {
        assert_eq!(ExecutionMode::Captured, ExecutionMode::Captured);
        assert_eq!(ExecutionMode::Interactive, ExecutionMode::Interactive);
        assert_ne!(ExecutionMode::Captured, ExecutionMode::Interactive);
    }

    #[test]
    fn test_is_success_true() {
        let result = CodeExecutionResult {
            script_path: PathBuf::from("test.py"),
            stdout: "ok".to_string(),
            stderr: String::new(),
            exit_code: Some(0),
        };
        assert!(result.is_success());
    }

    #[test]
    fn test_is_success_false_nonzero() {
        let result = CodeExecutionResult {
            script_path: PathBuf::from("test.py"),
            stdout: String::new(),
            stderr: "error".to_string(),
            exit_code: Some(1),
        };
        assert!(!result.is_success());
    }

    #[test]
    fn test_is_success_false_none() {
        let result = CodeExecutionResult {
            script_path: PathBuf::from("test.py"),
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
        };
        assert!(!result.is_success());
    }

    #[test]
    fn test_write_script() {
        let executor = CodeExecutor::new("test_write_script_dir").unwrap();
        let path = executor.write_script("print('hi')").unwrap();
        assert!(path.exists());
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "print('hi')");
        let _ = fs::remove_dir_all("test_write_script_dir");
    }

    #[test]
    fn test_syntax_check_valid() {
        let executor = CodeExecutor::new("test_syntax_valid").unwrap();
        let path = executor.write_script("print('hello')").unwrap();
        assert!(executor.syntax_check(&path).is_ok());
        let _ = fs::remove_dir_all("test_syntax_valid");
    }

    #[test]
    fn test_syntax_check_invalid() {
        let executor = CodeExecutor::new("test_syntax_invalid").unwrap();
        let path = executor.write_script("def foo(\n").unwrap();
        assert!(executor.syntax_check(&path).is_err());
        let _ = fs::remove_dir_all("test_syntax_invalid");
    }

    #[test]
    fn test_execution_timeout() {
        let executor = CodeExecutor::new("test_timeout_dir").unwrap();
        let path = executor.write_script("import time\ntime.sleep(10)").unwrap();
        let result = executor.execute_script(&path, ExecutionMode::Captured, 2).unwrap();
        assert!(!result.is_success());
        assert!(result.stderr.contains("timed out"));
        let _ = fs::remove_dir_all("test_timeout_dir");
    }
}
