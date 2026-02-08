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

/// Severity level for a lint diagnostic.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LintSeverity {
    Warning,
    Error,
}

/// A single diagnostic message from the linter.
#[derive(Debug, Clone)]
pub struct LintDiagnostic {
    pub message: String,
    pub severity: LintSeverity,
}

/// Result of running `ruff check` on a Python script.
#[derive(Debug)]
pub struct LintResult {
    /// True if no diagnostics at all.
    pub passed: bool,
    /// True if at least one diagnostic is an error (E/F rules).
    pub has_errors: bool,
    /// Individual diagnostic messages.
    pub diagnostics: Vec<LintDiagnostic>,
    /// Summary line from ruff (e.g. "Found 3 errors.").
    pub summary: String,
    /// Stderr output from ruff (internal errors, if any).
    pub stderr: String,
}

/// Severity level for a security diagnostic from bandit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SecuritySeverity {
    Low,
    Medium,
    High,
}

impl std::fmt::Display for SecuritySeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecuritySeverity::Low => write!(f, "LOW"),
            SecuritySeverity::Medium => write!(f, "MEDIUM"),
            SecuritySeverity::High => write!(f, "HIGH"),
        }
    }
}

/// A single diagnostic message from the security scanner.
#[derive(Debug, Clone)]
pub struct SecurityDiagnostic {
    /// Human-readable message (e.g. "Use of unsafe exec detected").
    pub message: String,
    /// Severity of the finding.
    pub severity: SecuritySeverity,
    /// Confidence level reported by bandit.
    pub confidence: SecuritySeverity,
    /// Bandit test ID (e.g. "B102").
    pub test_id: String,
    /// Line number in the script.
    pub line_number: u32,
}

/// Result of running `bandit` on a Python script.
#[derive(Debug)]
pub struct SecurityResult {
    /// True if no security findings at all.
    pub passed: bool,
    /// True if at least one finding has HIGH severity.
    pub has_high_severity: bool,
    /// Individual security findings.
    pub diagnostics: Vec<SecurityDiagnostic>,
    /// Summary string (e.g. "Found 2 issue(s)").
    pub summary: String,
    /// Any stderr output from bandit.
    pub stderr: String,
}

/// Responsible for writing Python scripts to disk and executing them,
/// either on the host or inside a Docker sandbox.
pub struct CodeExecutor {
    base_dir: PathBuf,
    use_docker: bool,
    use_venv: bool,
    python_executable: String,
}

impl CodeExecutor {
    /// Create a code executor.
    ///
    /// `base_dir`: directory where generated scripts are stored.
    /// `use_docker`: if true, scripts run inside the `python-sandbox` Docker container.
    /// `use_venv`: if true, each execution runs inside a temporary Python virtual environment.
    pub fn new(base_dir: &str, use_docker: bool, use_venv: bool, python_executable: &str) -> Result<Self> {
        let dir = PathBuf::from(base_dir);
        ensure_dir(&dir)?;
        Ok(Self { base_dir: dir, use_docker, use_venv, python_executable: python_executable.to_string() })
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

    // ── Virtual environment management ──────────────────────────────────

    /// Create a temporary Python virtual environment on the host.
    ///
    /// Returns `Some(path)` when `use_venv` is enabled and Docker is off,
    /// `None` when venv is disabled or Docker mode is active (Docker+venv
    /// creates the venv inline inside the container at execution time).
    pub fn create_venv(&self) -> Result<Option<PathBuf>> {
        if !self.use_venv {
            return Ok(None);
        }
        // In Docker+venv mode, the venv is created inside the container.
        if self.use_docker {
            return Ok(None);
        }

        let ts = Utc::now().format("%Y%m%d_%H%M%S_%3f");
        let venv_dir = std::env::temp_dir().join(format!("pymakebot_venv_{ts}"));

        let primary = self.python_executable.as_str();
        let python_cmds = [primary, "python"];
        let mut last_err: Option<anyhow::Error> = None;

        for cmd in python_cmds {
            let output = Command::new(cmd)
                .args(["-m", "venv"])
                .arg(&venv_dir)
                .output();

            match output {
                Ok(out) if out.status.success() => {
                    println!("✓ Virtual environment created at {}", venv_dir.display());
                    return Ok(Some(venv_dir));
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    last_err = Some(anyhow::anyhow!("venv creation failed with {}: {}", cmd, stderr));
                }
                Err(e) => {
                    last_err = Some(anyhow::anyhow!("Failed to run {} -m venv: {}", cmd, e));
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            anyhow::anyhow!("Could not create virtual environment with python/python3")
        }))
    }

    /// Return the Python interpreter path inside a host venv.
    fn venv_python(venv_path: &std::path::Path) -> PathBuf {
        if cfg!(windows) {
            venv_path.join("Scripts").join("python.exe")
        } else {
            // Try python3 first, then python (venv may create either or both)
            let python3 = venv_path.join("bin").join("python3");
            if python3.exists() {
                return python3;
            }
            venv_path.join("bin").join("python")
        }
    }

    /// Return the pip executable path inside a host venv.
    fn venv_pip(venv_path: &std::path::Path) -> PathBuf {
        if cfg!(windows) {
            venv_path.join("Scripts").join("pip.exe")
        } else {
            venv_path.join("bin").join("pip")
        }
    }

    /// Remove a temporary virtual environment directory.
    pub fn cleanup_venv(&self, venv_path: &std::path::Path) {
        if venv_path.exists() {
            match fs::remove_dir_all(venv_path) {
                Ok(()) => println!("✓ Virtual environment cleaned up"),
                Err(e) => eprintln!("Warning: failed to remove venv at {}: {}", venv_path.display(), e),
            }
        }
    }

    // ── Package installation ────────────────────────────────────────────

    /// Install Python packages using pip.
    ///
    /// * Host mode without venv: installs system-wide.
    /// * Host mode with venv: installs into the provided venv.
    /// * Docker mode without venv: commits packages into the Docker image.
    /// * Docker mode with venv: no-op — deps are installed inline at execution time.
    pub fn install_packages(&self, packages: &[String], venv: Option<&std::path::Path>) -> Result<()> {
        if packages.is_empty() {
            return Ok(());
        }

        // Docker+venv: deps will be installed inside the container at execution time
        if self.use_docker && self.use_venv {
            println!("ℹ  Dependencies ({}) will be installed in a container venv at execution time",
                packages.join(", "));
            return Ok(());
        }

        println!("Installing dependencies: {}", packages.join(", "));

        if self.use_docker {
            return self.install_packages_docker(packages);
        }

        if let Some(venv_path) = venv {
            return self.install_packages_venv(venv_path, packages);
        }

        self.install_packages_host(packages)
    }

    /// Install packages into a host-side virtual environment.
    fn install_packages_venv(&self, venv_path: &std::path::Path, packages: &[String]) -> Result<()> {
        let pip = Self::venv_pip(venv_path);
        let mut args = vec!["install".to_string(), "--quiet".to_string()];
        args.extend(packages.iter().cloned());

        let output = Command::new(&pip)
            .args(&args)
            .output()
            .with_context(|| format!("Failed to run pip in venv at {}", venv_path.display()))?;

        if output.status.success() {
            println!("✓ Dependencies installed in virtual environment");
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(anyhow::anyhow!("pip install failed in venv: {}", stderr))
        }
    }

    /// Install packages on the host via pip (system-wide).
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

    /// Install packages inside the Docker sandbox image (no venv).
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

    // ── Static analysis (linting) ───────────────────────────────────────

    /// Check whether `ruff` is available on PATH.
    pub fn check_linter_available() -> bool {
        Command::new("ruff")
            .arg("version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Run `ruff check` on a Python script and return structured results.
    ///
    /// Returns `Ok(LintResult)` with any diagnostics found.
    /// The caller decides whether warnings should block execution.
    pub fn lint_check(&self, path: &PathBuf) -> Result<LintResult> {
        let output = Command::new("ruff")
            .args(["check", "--output-format=concise", "--no-fix"])
            .arg(path)
            .output()
            .context("Failed to run ruff. Is it installed? (pip install ruff)")?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        // ruff exits 0 = clean, 1 = issues found, 2 = internal error
        let diagnostics: Vec<LintDiagnostic> = stdout
            .lines()
            .filter(|line| !line.trim().is_empty() && !line.starts_with("Found "))
            .map(|line| {
                // Try to parse severity from the rule code (E = error, W = warning, etc.)
                let severity = if line.contains(" F") || line.contains(" E") {
                    LintSeverity::Error
                } else {
                    LintSeverity::Warning
                };
                LintDiagnostic {
                    message: line.to_string(),
                    severity,
                }
            })
            .collect();

        let has_errors = diagnostics.iter().any(|d| d.severity == LintSeverity::Error);

        // Capture the "Found N ..." summary line if present
        let summary = stdout
            .lines()
            .find(|line| line.starts_with("Found "))
            .unwrap_or("")
            .to_string();

        Ok(LintResult {
            passed: diagnostics.is_empty(),
            has_errors,
            diagnostics,
            summary,
            stderr,
        })
    }

    // ── Static security analysis (bandit) ───────────────────────────────

    /// Check whether `bandit` is available on PATH.
    pub fn check_security_scanner_available() -> bool {
        Command::new("bandit")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Run `bandit` on a Python script and return structured security results.
    ///
    /// Uses JSON output for reliable parsing. Returns `Ok(SecurityResult)` with
    /// any findings. The caller decides whether high-severity findings should
    /// block execution.
    pub fn security_check(&self, path: &PathBuf) -> Result<SecurityResult> {
        let output = Command::new("bandit")
            .args(["-f", "json", "-q"])
            .arg(path)
            .output()
            .context("Failed to run bandit. Is it installed? (pip install bandit)")?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        // bandit exits 0 = clean, 1 = issues found
        // Parse JSON output
        let diagnostics = Self::parse_bandit_json(&stdout);
        let has_high_severity = diagnostics.iter().any(|d| d.severity == SecuritySeverity::High);
        let count = diagnostics.len();
        let summary = if count == 0 {
            String::new()
        } else {
            let high = diagnostics.iter().filter(|d| d.severity == SecuritySeverity::High).count();
            let med = diagnostics.iter().filter(|d| d.severity == SecuritySeverity::Medium).count();
            let low = diagnostics.iter().filter(|d| d.severity == SecuritySeverity::Low).count();
            format!(
                "Found {} issue(s): {} high, {} medium, {} low severity",
                count, high, med, low
            )
        };

        Ok(SecurityResult {
            passed: diagnostics.is_empty(),
            has_high_severity,
            diagnostics,
            summary,
            stderr,
        })
    }

    /// Parse bandit JSON output into a list of security diagnostics.
    fn parse_bandit_json(json_str: &str) -> Vec<SecurityDiagnostic> {
        // bandit JSON format: { "results": [ { "issue_severity": "HIGH", ... } ], ... }
        let parsed: serde_json::Value = match serde_json::from_str(json_str) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        let results = match parsed.get("results").and_then(|r| r.as_array()) {
            Some(arr) => arr,
            None => return Vec::new(),
        };

        results
            .iter()
            .filter_map(|item| {
                let severity_str = item.get("issue_severity")?.as_str()?;
                let confidence_str = item.get("issue_confidence")?.as_str()?;
                let test_id = item.get("test_id")?.as_str()?.to_string();
                let issue_text = item.get("issue_text")?.as_str()?.to_string();
                let line_number = item.get("line_number")?.as_u64()? as u32;

                let severity = match severity_str {
                    "HIGH" => SecuritySeverity::High,
                    "MEDIUM" => SecuritySeverity::Medium,
                    _ => SecuritySeverity::Low,
                };
                let confidence = match confidence_str {
                    "HIGH" => SecuritySeverity::High,
                    "MEDIUM" => SecuritySeverity::Medium,
                    _ => SecuritySeverity::Low,
                };

                Some(SecurityDiagnostic {
                    message: format!("[{}] {} (line {})", test_id, issue_text, line_number),
                    severity,
                    confidence,
                    test_id,
                    line_number,
                })
            })
            .collect()
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
        self.execute_script(&script_path, mode, 0, None, &[]) // 0 = no timeout
    }

    /// Execute a previously generated script by path.
    pub fn run_existing_script(
        &self,
        script_path: &str,
        mode: ExecutionMode,
        timeout_secs: u64,
        venv: Option<&std::path::Path>,
        deps: &[String],
    ) -> Result<CodeExecutionResult> {
        let path = PathBuf::from(script_path);
        if !path.exists() {
            return Err(anyhow::anyhow!("Script not found: {}", script_path));
        }
        self.execute_script(&path, mode, timeout_secs, venv, deps)
    }

    /// Execute a Python script. `timeout_secs == 0` means no timeout.
    /// Timeout only applies to `Captured` mode.
    ///
    /// * `venv` — path to a host-side venv (used in host+venv mode).
    /// * `deps` — packages to install in a Docker venv (used in Docker+venv mode).
    ///
    /// When `self.use_docker` is true, runs inside the `python-sandbox` container.
    pub fn execute_script(
        &self,
        script_path: &PathBuf,
        mode: ExecutionMode,
        timeout_secs: u64,
        venv: Option<&std::path::Path>,
        deps: &[String],
    ) -> Result<CodeExecutionResult> {
        if self.use_docker {
            self.execute_script_docker(script_path, mode, timeout_secs, deps)
        } else {
            self.execute_script_host(script_path, mode, timeout_secs, venv)
        }
    }

    /// Execute a script inside the Docker sandbox container.
    ///
    /// When `use_venv` is enabled, creates a temporary venv inside the container,
    /// installs `deps`, and runs the script — all in a single ephemeral `docker run`.
    /// This avoids mutating the base Docker image.
    fn execute_script_docker(
        &self,
        script_path: &PathBuf,
        mode: ExecutionMode,
        timeout_secs: u64,
        deps: &[String],
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

        // When venv is enabled, build a shell command that creates a venv,
        // installs dependencies, and runs the script — all in one ephemeral container.
        let use_venv_in_docker = self.use_venv;

        // Only enforce network isolation when no packages need downloading.
        // When deps are present the user has already approved the install,
        // so pip needs network access inside the container.
        let needs_network = use_venv_in_docker && !deps.is_empty();

        // Build the entrypoint command for venv mode
        let venv_shell_cmd = if use_venv_in_docker {
            let mut parts = vec![
                "python3 -m venv /tmp/venv".to_string(),
            ];
            if !deps.is_empty() {
                parts.push(format!(
                    "/tmp/venv/bin/pip install --quiet {}",
                    deps.join(" ")
                ));
            }
            parts.push(format!("/tmp/venv/bin/python3 {}", script_in_container));
            Some(parts.join(" && "))
        } else {
            None
        };

        match mode {
            ExecutionMode::Interactive => {
                let mut cmd = Command::new("docker");
                cmd.args([
                    "run", "--rm",
                    "-i",
                    "-v", &volume_mount,
                ]);
                if !needs_network {
                    cmd.args(["--network", "none"]);
                }

                if let Some(ref shell_cmd) = venv_shell_cmd {
                    // Venv mode: need root to create venv, run via bash
                    cmd.args(["--user", "root", DOCKER_IMAGE, "bash", "-c", shell_cmd]);
                } else {
                    cmd.args([DOCKER_IMAGE, "python3", &script_in_container]);
                }

                let child = cmd
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
                let mut cmd = Command::new("docker");
                cmd.args([
                    "run", "--rm",
                    "-v", &volume_mount,
                ]);
                if !needs_network {
                    cmd.args(["--network", "none"]);
                }

                if let Some(ref shell_cmd) = venv_shell_cmd {
                    cmd.args(["--user", "root", DOCKER_IMAGE, "bash", "-c", shell_cmd]);
                } else {
                    cmd.args([DOCKER_IMAGE, "python3", &script_in_container]);
                }

                let child = cmd
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
    /// When `venv` is provided, uses the venv's Python interpreter instead.
    fn execute_script_host(
        &self,
        script_path: &PathBuf,
        mode: ExecutionMode,
        timeout_secs: u64,
        venv: Option<&std::path::Path>,
    ) -> Result<CodeExecutionResult> {
        // If a venv is available, use its python directly (no fallback needed)
        if let Some(venv_path) = venv {
            let python = Self::venv_python(venv_path);
            let python_str = python.to_str()
                .ok_or_else(|| anyhow::anyhow!("Venv python path is not valid UTF-8"))?;
            return self.execute_with_interpreter(python_str, script_path, mode, timeout_secs);
        }

        // No venv — fall back through system interpreters
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

    /// Execute a script with a specific interpreter (used for venv python path).
    fn execute_with_interpreter(
        &self,
        interpreter: &str,
        script_path: &PathBuf,
        mode: ExecutionMode,
        timeout_secs: u64,
    ) -> Result<CodeExecutionResult> {
        match mode {
            ExecutionMode::Interactive => {
                let child = Command::new(interpreter)
                    .arg(script_path)
                    .stdin(Stdio::inherit())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .with_context(|| format!("Failed to spawn venv python: {}", interpreter))?;

                let status = child.wait_with_output()
                    .context("Failed to wait for venv process")?;
                Ok(CodeExecutionResult {
                    script_path: script_path.clone(),
                    stdout: String::from("[Interactive mode - output displayed directly]"),
                    stderr: String::new(),
                    exit_code: status.status.code(),
                })
            }
            ExecutionMode::Captured => {
                let mut process = Command::new(interpreter)
                    .arg(script_path)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .with_context(|| format!("Failed to spawn venv python: {}", interpreter))?;

                if timeout_secs > 0 {
                    let timeout = Duration::from_secs(timeout_secs);
                    match process.wait_timeout(timeout)
                        .context("Failed to wait for venv process")?
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
                                    "Process timed out after {} seconds. \
                                     You can increase this with execution_timeout_secs in pymakebot.toml",
                                    timeout_secs
                                ),
                                exit_code: None,
                            })
                        }
                    }
                } else {
                    let output = process.wait_with_output()
                        .context("Failed to wait for venv process")?;
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
        }
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
    use std::sync::Mutex;

    /// Mutex to serialize tests that create real Python virtual environments.
    /// Parallel `python3 -m venv` calls can interfere with each other on some
    /// Python distributions (e.g. Anaconda), causing missing symlinks.
    static VENV_LOCK: Mutex<()> = Mutex::new(());

    /// Helper: create an executor with Docker disabled, venv disabled (host mode).
    fn host_executor(dir: &str) -> CodeExecutor {
        CodeExecutor::new(dir, false, false, "python3").unwrap()
    }

    #[test]
    fn test_executor_creation() {
        let temp_dir = "test_executor_temp";
        let executor = CodeExecutor::new(temp_dir, false, false, "python3");
        assert!(executor.is_ok());
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_executor_creation_docker_flag() {
        let temp_dir = "test_executor_docker_flag";
        let executor = CodeExecutor::new(temp_dir, true, false, "python3").unwrap();
        assert!(executor.use_docker);
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_executor_creation_venv_flag() {
        let temp_dir = "test_executor_venv_flag";
        let executor = CodeExecutor::new(temp_dir, false, true, "python3").unwrap();
        assert!(executor.use_venv);
        assert!(!executor.use_docker);
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
        let result = executor.install_packages(&[], None);
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
        let result = executor.execute_script(&path, ExecutionMode::Captured, 2, None, &[]).unwrap();
        assert!(!result.is_success());
        assert!(result.stderr.contains("timed out"));
        let _ = fs::remove_dir_all("test_timeout_dir");
    }

    #[test]
    fn test_docker_image_constant() {
        // Ensure the constant matches what the Dockerfile builds
        assert_eq!(DOCKER_IMAGE, "python-sandbox");
    }

    #[test]
    fn test_create_venv_disabled() {
        // When use_venv is false, create_venv returns None
        let executor = host_executor("test_venv_disabled");
        let result = executor.create_venv().unwrap();
        assert!(result.is_none());
        let _ = fs::remove_dir_all("test_venv_disabled");
    }

    #[test]
    fn test_create_venv_docker_mode() {
        // When use_docker is true (even with use_venv), create_venv returns None
        // because venv is created inside the container at execution time
        let temp_dir = "test_venv_docker_mode";
        let executor = CodeExecutor::new(temp_dir, true, true, "python3").unwrap();
        let result = executor.create_venv().unwrap();
        assert!(result.is_none());
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_create_and_cleanup_venv() {
        let _lock = VENV_LOCK.lock().unwrap();
        // When use_venv is true and Docker is off, create_venv makes a real venv
        let temp_dir = "test_create_cleanup_venv";
        let executor = CodeExecutor::new(temp_dir, false, true, "python3").unwrap();
        let venv = executor.create_venv().unwrap();
        assert!(venv.is_some());
        let venv_path = venv.unwrap();
        assert!(venv_path.exists());
        // Check the venv has a python3 binary
        let python = CodeExecutor::venv_python(&venv_path);
        assert!(python.exists(), "venv python not found at {:?} (checked python3 and python)", python);
        // Clean up
        executor.cleanup_venv(&venv_path);
        assert!(!venv_path.exists());
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_execute_in_venv() {
        let _lock = VENV_LOCK.lock().unwrap();
        // Create a venv, then execute a simple script in it
        let temp_dir = "test_execute_in_venv";
        let executor = CodeExecutor::new(temp_dir, false, true, "python3").unwrap();
        let venv = executor.create_venv().unwrap();
        assert!(venv.is_some());
        let venv_path = venv.as_deref().unwrap();
        let path = executor.write_script("import sys; print(sys.prefix)").unwrap();
        let result = executor.execute_script(&path, ExecutionMode::Captured, 5, Some(venv_path), &[]).unwrap();
        assert!(result.is_success());
        // The output should mention the venv path
        assert!(!result.stdout.trim().is_empty());
        executor.cleanup_venv(venv_path);
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_install_packages_docker_venv_noop() {
        // Docker+venv mode: install_packages is a no-op
        let temp_dir = "test_docker_venv_noop";
        let executor = CodeExecutor::new(temp_dir, true, true, "python3").unwrap();
        let result = executor.install_packages(&["requests".to_string()], None);
        assert!(result.is_ok());
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_check_linter_available() {
        // Should return a bool without panicking
        let _available = CodeExecutor::check_linter_available();
    }

    #[test]
    fn test_lint_check_clean_code() {
        if !CodeExecutor::check_linter_available() {
            // Skip if ruff is not installed
            return;
        }
        let temp_dir = "test_lint_clean";
        let executor = host_executor(temp_dir);
        let path = executor.write_script("x = 1\nprint(x)\n").unwrap();
        let result = executor.lint_check(&path).unwrap();
        assert!(result.passed);
        assert!(!result.has_errors);
        assert!(result.diagnostics.is_empty());
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_lint_check_with_issues() {
        if !CodeExecutor::check_linter_available() {
            return;
        }
        let temp_dir = "test_lint_issues";
        let executor = host_executor(temp_dir);
        // Import os but never use it — ruff should flag F401 (unused import)
        let path = executor.write_script("import os\nprint('hello')\n").unwrap();
        let result = executor.lint_check(&path).unwrap();
        assert!(!result.passed, "Expected lint issues for unused import");
        assert!(!result.diagnostics.is_empty());
        // Check that at least one diagnostic mentions F401 or the unused import
        let has_unused = result.diagnostics.iter().any(|d| d.message.contains("F401") || d.message.contains("unused"));
        assert!(has_unused, "Expected F401 unused import diagnostic, got: {:?}", result.diagnostics);
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_lint_check_severity_error() {
        if !CodeExecutor::check_linter_available() {
            return;
        }
        let temp_dir = "test_lint_severity";
        let executor = host_executor(temp_dir);
        // Undefined name (F821) is an error-level diagnostic
        let path = executor.write_script("print(undefined_variable)\n").unwrap();
        let result = executor.lint_check(&path).unwrap();
        assert!(result.has_errors, "Expected lint errors for undefined name");
        let has_f_error = result.diagnostics.iter().any(|d| d.severity == LintSeverity::Error);
        assert!(has_f_error);
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_lint_result_summary() {
        if !CodeExecutor::check_linter_available() {
            return;
        }
        let temp_dir = "test_lint_summary";
        let executor = host_executor(temp_dir);
        let path = executor.write_script("import os\nimport sys\nprint('hello')\n").unwrap();
        let result = executor.lint_check(&path).unwrap();
        if !result.passed {
            // ruff prints "Found N error(s)." summary
            assert!(!result.summary.is_empty(), "Expected a summary line from ruff");
        }
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_check_security_scanner_available() {
        // Should return a bool without panicking
        let _available = CodeExecutor::check_security_scanner_available();
    }

    #[test]
    fn test_security_check_clean_code() {
        if !CodeExecutor::check_security_scanner_available() {
            // Skip if bandit is not installed
            return;
        }
        let temp_dir = "test_security_clean";
        let executor = host_executor(temp_dir);
        let path = executor.write_script("x = 1\nprint(x)\n").unwrap();
        let result = executor.security_check(&path).unwrap();
        assert!(result.passed, "Expected no security issues for clean code");
        assert!(!result.has_high_severity);
        assert!(result.diagnostics.is_empty());
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_security_check_with_issues() {
        if !CodeExecutor::check_security_scanner_available() {
            return;
        }
        let temp_dir = "test_security_issues";
        let executor = host_executor(temp_dir);
        // subprocess call with shell=True — bandit flags this as B602
        let code = "import subprocess\nsubprocess.call('ls', shell=True)\n";
        let path = executor.write_script(code).unwrap();
        let result = executor.security_check(&path).unwrap();
        assert!(!result.passed, "Expected security issues for shell=True subprocess");
        assert!(!result.diagnostics.is_empty());
        // Check that at least one diagnostic mentions shell or subprocess
        let has_relevant = result.diagnostics.iter().any(|d|
            d.test_id.starts_with("B") || d.message.contains("shell")
        );
        assert!(has_relevant, "Expected bandit finding, got: {:?}", result.diagnostics);
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_security_check_high_severity() {
        if !CodeExecutor::check_security_scanner_available() {
            return;
        }
        let temp_dir = "test_security_high";
        let executor = host_executor(temp_dir);
        // exec() is flagged as B102 with HIGH severity
        let code = "exec('print(1)')\n";
        let path = executor.write_script(code).unwrap();
        let result = executor.security_check(&path).unwrap();
        // exec() should trigger at least one finding
        if !result.passed {
            let has_finding = result.diagnostics.iter().any(|d| !d.test_id.is_empty());
            assert!(has_finding, "Expected a bandit test ID");
        }
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_security_result_summary() {
        if !CodeExecutor::check_security_scanner_available() {
            return;
        }
        let temp_dir = "test_security_summary";
        let executor = host_executor(temp_dir);
        let code = "import subprocess\nsubprocess.call('ls', shell=True)\n";
        let path = executor.write_script(code).unwrap();
        let result = executor.security_check(&path).unwrap();
        if !result.passed {
            assert!(!result.summary.is_empty(), "Expected a summary string");
            assert!(result.summary.contains("issue"), "Summary should mention issue count");
        }
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_parse_bandit_json_empty() {
        let result = CodeExecutor::parse_bandit_json("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_bandit_json_no_results() {
        let json = r#"{"results": [], "errors": []}"#;
        let result = CodeExecutor::parse_bandit_json(json);
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_bandit_json_with_results() {
        let json = r#"{
            "results": [{
                "issue_severity": "HIGH",
                "issue_confidence": "HIGH",
                "issue_text": "Use of exec detected.",
                "test_id": "B102",
                "line_number": 1
            }]
        }"#;
        let result = CodeExecutor::parse_bandit_json(json);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].severity, SecuritySeverity::High);
        assert_eq!(result[0].confidence, SecuritySeverity::High);
        assert_eq!(result[0].test_id, "B102");
        assert_eq!(result[0].line_number, 1);
        assert!(result[0].message.contains("exec"));
    }
}
