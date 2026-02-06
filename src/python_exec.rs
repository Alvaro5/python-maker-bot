use crate::utils::{ensure_dir, extract_imports, is_stdlib};
use anyhow::{Context, Result};
use chrono::Utc;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

const DOCKER_IMAGE: &str = "python-sandbox";

/// Execution mode for Python scripts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExecutionMode {
    /// Default mode: capture stdout/stderr
    Captured,
    /// Interactive mode: inherits stdio (for games, user input, GUIs)
    Interactive,
}

/// Result of a Python script execution.
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

/// Responsible for writing Python scripts to disk and executing them,
/// either on the host or inside a Docker sandbox.
pub struct CodeExecutor {
    base_dir: PathBuf,
    use_docker: bool,
    python_executable: String,
}

impl CodeExecutor {
    /// Create a code executor.
    ///
    /// `base_dir`: directory where generated scripts are stored.
    /// `use_docker`: if true, scripts run inside the `python-sandbox` Docker container.
    pub fn new(base_dir: &str, use_docker: bool, python_executable: &str) -> Result<Self> {
        let dir = PathBuf::from(base_dir);
        ensure_dir(&dir)?;
        Ok(Self { base_dir: dir, use_docker, python_executable: python_executable.to_string() })
    }

    /// Check whether Docker is available and the sandbox image exists.
    /// Returns Ok(()) on success or an error describing what is missing.
    pub fn check_docker_available() -> Result<()> {
        // Check that the docker CLI is reachable
        let version = Command::new("docker")
            .arg("version")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .status();

        match version {
            Ok(s) if s.success() => {}
            Ok(_) => return Err(anyhow::anyhow!("Docker daemon is not running")),
            Err(e) => return Err(anyhow::anyhow!("Docker CLI not found: {}", e)),
        }

        // Check that the sandbox image exists
        let inspect = Command::new("docker")
            .args(["image", "inspect", DOCKER_IMAGE])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("Failed to run docker image inspect")?;

        if !inspect.success() {
            return Err(anyhow::anyhow!(
                "Docker image '{}' not found. Build it with: docker build -t {} .",
                DOCKER_IMAGE, DOCKER_IMAGE
            ));
        }

        Ok(())
    }

    /// Detect non-standard library dependencies in Python code
    pub fn detect_dependencies(&self, code: &str) -> Vec<String> {
        let all_imports = extract_imports(code);
        all_imports
            .into_iter()
            .filter(|pkg| !is_stdlib(pkg))
            .collect()
    }

    /// Install Python packages using pip (host or Docker).
    pub fn install_packages(&self, packages: &[String]) -> Result<()> {
        if packages.is_empty() {
            return Ok(());
        }

        println!("Installing dependencies: {}", packages.join(", "));

        if self.use_docker {
            return self.install_packages_docker(packages);
        }

        self.install_packages_host(packages)
    }

    /// Install packages on the host via pip.
    fn install_packages_host(&self, packages: &[String]) -> Result<()> {
        let primary = self.python_executable.as_str();
        let python_cmds = [primary, "python"];
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

    /// Install packages inside the Docker sandbox image.
    /// We run `pip install` inside a temporary container based on the sandbox
    /// image, then commit the result back so subsequent runs have the packages.
    fn install_packages_docker(&self, packages: &[String]) -> Result<()> {
        let container_name = format!("pymakebot-pip-{}", std::process::id());

        let mut args = vec![
            "run".to_string(),
            "--name".to_string(),
            container_name.clone(),
            "--user".to_string(),
            "root".to_string(),  // need root to pip install
            DOCKER_IMAGE.to_string(),
            "pip".to_string(),
            "install".to_string(),
            "--quiet".to_string(),
        ];
        args.extend(packages.iter().cloned());

        let output = Command::new("docker")
            .args(&args)
            .output()
            .context("Failed to run pip install inside Docker")?;

        if output.status.success() {
            // Commit the container with installed packages back to the image
            let commit = Command::new("docker")
                .args(["commit", &container_name, DOCKER_IMAGE])
                .output()
                .context("Failed to commit Docker container after pip install")?;

            // Clean up the stopped container
            let _ = Command::new("docker")
                .args(["rm", &container_name])
                .output();

            if commit.status.success() {
                println!("✓ Dependencies installed successfully (Docker)");
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&commit.stderr);
                Err(anyhow::anyhow!("Failed to commit Docker image after pip install: {}", stderr))
            }
        } else {
            // Clean up the failed container
            let _ = Command::new("docker")
                .args(["rm", &container_name])
                .output();

            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(anyhow::anyhow!("pip install failed inside Docker: {}", stderr))
        }
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
        let primary = self.python_executable.as_str();
        let python_cmds = [primary, "python"];
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

    /// Write and execute a Python script with the specified execution mode.
    pub fn write_and_run_with_mode(&self, code: &str, mode: ExecutionMode) -> Result<CodeExecutionResult> {
        let script_path = self.write_script(code)?;
        self.execute_script(&script_path, mode, 0) // 0 = no timeout
    }

    /// Execute a previously generated script by path.
    pub fn run_existing_script(&self, script_path: &str, mode: ExecutionMode, timeout_secs: u64) -> Result<CodeExecutionResult> {
        let path = PathBuf::from(script_path);
        if !path.exists() {
            return Err(anyhow::anyhow!("Script not found: {}", script_path));
        }
        self.execute_script(&path, mode, timeout_secs)
    }

    /// Execute a Python script. `timeout_secs == 0` means no timeout.
    /// Timeout only applies to `Captured` mode.
    /// When `self.use_docker` is true, runs inside the `python-sandbox` container.
    pub fn execute_script(&self, script_path: &PathBuf, mode: ExecutionMode, timeout_secs: u64) -> Result<CodeExecutionResult> {
        if self.use_docker {
            self.execute_script_docker(script_path, mode, timeout_secs)
        } else {
            self.execute_script_host(script_path, mode, timeout_secs)
        }
    }

    /// Execute a script inside the Docker sandbox container.
    fn execute_script_docker(
        &self,
        script_path: &PathBuf,
        mode: ExecutionMode,
        timeout_secs: u64,
    ) -> Result<CodeExecutionResult> {
        let absolute_path = std::fs::canonicalize(script_path)
            .with_context(|| format!("Could not resolve path: {:?}", script_path))?;
        let parent_dir = absolute_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Script has no parent directory"))?
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Script parent path is not valid UTF-8"))?;
        let filename = absolute_path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("Script has no filename"))?
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Script filename is not valid UTF-8"))?;

        let volume_mount = format!("{}:/home/sandboxuser/scripts:ro", parent_dir);
        let script_in_container = format!("/home/sandboxuser/scripts/{}", filename);

        match mode {
            ExecutionMode::Interactive => {
                // Interactive Docker: inherit stdio, add -it flags, no timeout
                let child = Command::new("docker")
                    .args([
                        "run", "--rm",
                        "-i",
                        "-v", &volume_mount,
                        "--network", "none",
                        DOCKER_IMAGE,
                        "python3", &script_in_container,
                    ])
                    .stdin(Stdio::inherit())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit())
                    .spawn();

                match child {
                    Ok(mut process) => {
                        let status = process.wait()
                            .context("Failed to wait for Docker process")?;
                        Ok(CodeExecutionResult {
                            script_path: script_path.clone(),
                            stdout: String::from("[Interactive mode - output displayed directly]"),
                            stderr: String::new(),
                            exit_code: status.code(),
                        })
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to spawn Docker interactive process: {}", e)),
                }
            }
            ExecutionMode::Captured => {
                let child = Command::new("docker")
                    .args([
                        "run", "--rm",
                        "-v", &volume_mount,
                        "--network", "none",
                        DOCKER_IMAGE,
                        "python3", &script_in_container,
                    ])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn();

                match child {
                    Ok(mut process) => {
                        if timeout_secs > 0 {
                            let timeout = Duration::from_secs(timeout_secs);
                            match process.wait_timeout(timeout)
                                .context("Failed to wait for Docker process")?
                            {
                                Some(status) => {
                                    let stdout = read_pipe(process.stdout.take());
                                    let stderr = read_pipe(process.stderr.take());
                                    Ok(CodeExecutionResult {
                                        script_path: script_path.clone(),
                                        stdout,
                                        stderr,
                                        exit_code: status.code(),
                                    })
                                }
                                None => {
                                    let _ = process.kill();
                                    let _ = process.wait();
                                    Ok(CodeExecutionResult {
                                        script_path: script_path.clone(),
                                        stdout: String::new(),
                                        stderr: format!(
                                            "Process timed out after {} seconds (Docker). \
                                             You can increase this with execution_timeout_secs in pymakebot.toml",
                                            timeout_secs
                                        ),
                                        exit_code: None,
                                    })
                                }
                            }
                        } else {
                            let output = process.wait_with_output()
                                .context("Failed to wait for Docker process")?;
                            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                            Ok(CodeExecutionResult {
                                script_path: script_path.clone(),
                                stdout,
                                stderr,
                                exit_code: output.status.code(),
                            })
                        }
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to spawn Docker process: {}", e)),
                }
            }
        }
    }

    /// Execute a script directly on the host with python3/python fallback.
    fn execute_script_host(
        &self,
        script_path: &PathBuf,
        mode: ExecutionMode,
        timeout_secs: u64,
    ) -> Result<CodeExecutionResult> {
        let primary = self.python_executable.as_str();
        let python_cmds = [primary, "python"];
        let mut last_err: Option<anyhow::Error> = None;

        for cmd in python_cmds {
            match mode {
                ExecutionMode::Interactive => {
                    // Interactive: inherit stdin/stdout/stderr, no timeout
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

    /// Helper: create an executor with Docker disabled (host mode).
    fn host_executor(dir: &str) -> CodeExecutor {
        CodeExecutor::new(dir, false, "python3").unwrap()
    }

    #[test]
    fn test_executor_creation() {
        let temp_dir = "test_executor_temp";
        let executor = CodeExecutor::new(temp_dir, false, "python3");
        assert!(executor.is_ok());
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_executor_creation_docker_flag() {
        let temp_dir = "test_executor_docker_flag";
        let executor = CodeExecutor::new(temp_dir, true, "python3").unwrap();
        assert!(executor.use_docker);
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_detect_dependencies_stdlib_only() {
        let executor = host_executor("test_temp");
        let code = "import os\nimport sys\nfrom pathlib import Path";
        let deps = executor.detect_dependencies(code);
        assert!(deps.is_empty());
        let _ = fs::remove_dir_all("test_temp");
    }

    #[test]
    fn test_detect_dependencies_third_party() {
        let executor = host_executor("test_temp");
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
        let executor = host_executor("test_temp");
        let code = "import os\nimport numpy\nimport sys\nfrom flask import Flask";
        let deps = executor.detect_dependencies(code);
        assert_eq!(deps.len(), 2);
        assert!(deps.contains(&"numpy".to_string()));
        assert!(deps.contains(&"flask".to_string()));
        let _ = fs::remove_dir_all("test_temp");
    }

    #[test]
    fn test_write_and_run_simple_script() {
        let executor = host_executor("test_generated_simple");
        let code = "print('Hello, Test!')";

        let result = executor.write_and_run(code);
        assert!(result.is_ok());

        let output = result.unwrap();
        let script_exists = output.script_path.exists();
        assert!(!output.stdout.is_empty() || !output.stderr.is_empty());
        assert!(script_exists);

        let _ = fs::remove_dir_all("test_generated_simple");
    }

    #[test]
    fn test_write_and_run_with_calculation() {
        let executor = host_executor("test_generated_calc");
        let code = "result = 2 + 2\nprint(f'Result: {result}')";

        let result = executor.write_and_run(code);
        assert!(result.is_ok());

        let output = result.unwrap();
        assert!(!output.stdout.is_empty() || !output.stderr.is_empty());

        let _ = fs::remove_dir_all("test_generated_calc");
    }

    #[test]
    fn test_write_and_run_error_script() {
        let executor = host_executor("test_generated_error");
        let code = "print(undefined_variable)";

        let result = executor.write_and_run(code);
        assert!(result.is_ok());

        let output = result.unwrap();
        let script_exists = output.script_path.exists();
        assert!(script_exists);

        let _ = fs::remove_dir_all("test_generated_error");
    }

    #[test]
    fn test_install_packages_empty_list() {
        let executor = host_executor("test_temp");
        let result = executor.install_packages(&[]);
        assert!(result.is_ok());
        let _ = fs::remove_dir_all("test_temp");
    }

    #[test]
    fn test_needs_interactive_mode_pygame() {
        let executor = host_executor("test_temp");
        let code = "import pygame\npygame.init()";
        assert!(executor.needs_interactive_mode(code));
        let _ = fs::remove_dir_all("test_temp");
    }

    #[test]
    fn test_needs_interactive_mode_input() {
        let executor = host_executor("test_temp");
        let code = "name = input('Enter your name: ')";
        assert!(executor.needs_interactive_mode(code));
        let _ = fs::remove_dir_all("test_temp");
    }

    #[test]
    fn test_needs_interactive_mode_simple_script() {
        let executor = host_executor("test_temp");
        let code = "print('Hello, World!')";
        assert!(!executor.needs_interactive_mode(code));
        let _ = fs::remove_dir_all("test_temp");
    }

    #[test]
    fn test_needs_interactive_mode_matplotlib() {
        let executor = host_executor("test_temp");
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
        let executor = host_executor("test_write_script_dir");
        let path = executor.write_script("print('hi')").unwrap();
        assert!(path.exists());
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "print('hi')");
        let _ = fs::remove_dir_all("test_write_script_dir");
    }

    #[test]
    fn test_syntax_check_valid() {
        let executor = host_executor("test_syntax_valid");
        let path = executor.write_script("print('hello')").unwrap();
        assert!(executor.syntax_check(&path).is_ok());
        let _ = fs::remove_dir_all("test_syntax_valid");
    }

    #[test]
    fn test_syntax_check_invalid() {
        let executor = host_executor("test_syntax_invalid");
        let path = executor.write_script("def foo(\n").unwrap();
        assert!(executor.syntax_check(&path).is_err());
        let _ = fs::remove_dir_all("test_syntax_invalid");
    }

    #[test]
    fn test_execution_timeout() {
        let executor = host_executor("test_timeout_dir");
        let path = executor.write_script("import time\ntime.sleep(10)").unwrap();
        let result = executor.execute_script(&path, ExecutionMode::Captured, 2).unwrap();
        assert!(!result.is_success());
        assert!(result.stderr.contains("timed out"));
        let _ = fs::remove_dir_all("test_timeout_dir");
    }

    #[test]
    fn test_docker_image_constant() {
        // Ensure the constant matches what the Dockerfile builds
        assert_eq!(DOCKER_IMAGE, "python-sandbox");
    }
}
