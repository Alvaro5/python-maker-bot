use anyhow::{anyhow, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

// Cached regexes — compiled once, reused across all calls
static CODE_BLOCK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"```\s*(?:python)?\s*([\s\S]*?)\s*```").unwrap());
static INCOMPLETE_BLOCK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"```\s*(?:python)?\s*\n([\s\S]*)$").unwrap());
static IMPORT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^import\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap());
static FROM_IMPORT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^from\s+([a-zA-Z_][a-zA-Z0-9_]*)\s+import").unwrap());

pub fn ensure_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::create_dir_all(path)
            .with_context(|| format!("Failed to create directory {:?}", path))?;
    }
    Ok(())
}

/// Find the largest char boundary in `s` that is <= `max_bytes`.
/// Safe for slicing: `&s[..find_char_boundary(s, max_bytes)]` never panics.
pub fn find_char_boundary(s: &str, max_bytes: usize) -> usize {
    if max_bytes >= s.len() {
        return s.len();
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !s.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

/// Extract Python code from a response that might contain markdown code blocks
pub fn extract_python_code(response: &str) -> String {
    // Find all complete code blocks and concatenate them
    let mut all_code = String::new();
    for capture in CODE_BLOCK_RE.captures_iter(response) {
        if let Some(code) = capture.get(1) {
            let code_str = code.as_str().trim();
            if !code_str.is_empty() && !is_just_markdown_text(code_str) {
                if !all_code.is_empty() {
                    all_code.push_str("\n\n");
                }
                all_code.push_str(code_str);
            }
        }
    }

    if !all_code.is_empty() {
        return all_code;
    }

    // If no complete blocks, try to extract from incomplete/truncated response
    // Pattern: ```python\n...code... (no closing backticks)
    if let Some(capture) = INCOMPLETE_BLOCK_RE.captures(response) {
        if let Some(code) = capture.get(1) {
            let code_str = code.as_str().trim();
            if !code_str.is_empty() && !is_just_markdown_text(code_str) {
                return code_str.to_string();
            }
        }
    }

    // If no markdown block found, clean up markdown artifacts and return
    let cleaned = clean_markdown_artifacts(response.trim());

    // If the result is mostly markdown text, return a helpful comment
    if is_just_markdown_text(&cleaned) {
        return "# No Python code was generated.\n# Please try rephrasing your request or use /refine to ask for actual code.".to_string();
    }

    cleaned
}

/// Check if text is just markdown explanations without actual code
fn is_just_markdown_text(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return true;
    }

    // Count lines that look like code vs markdown text
    let mut code_lines = 0;
    let mut text_lines = 0;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Markdown indicators (explicit parentheses for clarity)
        if trimmed.starts_with("###")
            || trimmed.starts_with("##")
            || (trimmed.starts_with("#") && !trimmed.contains("=") && !trimmed.contains("import"))
            || trimmed.starts_with("Here is")
            || trimmed.starts_with("Step ")
            || trimmed.starts_with("The ")
            || trimmed.contains("code for")
        {
            text_lines += 1;
        } else if trimmed.contains("def ")
            || trimmed.contains("class ")
            || trimmed.contains("import ")
            || trimmed.contains("=")
            || (trimmed.contains("(") && trimmed.contains(")"))
        {
            code_lines += 1;
        }
    }

    // If mostly text or no code at all, it's just markdown
    text_lines > code_lines || code_lines == 0
}

/// Remove common markdown artifacts from text
fn clean_markdown_artifacts(text: &str) -> String {
    let mut result = String::new();

    for line in text.lines() {
        let trimmed = line.trim();

        // Skip obvious markdown headings and explanations
        if trimmed.starts_with("###")
            || trimmed.starts_with("##")
            || (trimmed.starts_with("Here is") && trimmed.contains(":"))
            || (trimmed.starts_with("Step ") && trimmed.contains(":"))
        {
            continue;
        }

        result.push_str(line);
        result.push('\n');
    }

    result.trim().to_string()
}

/// Extract all import statements from Python code
/// Returns a list of package names (without submodules)
pub fn extract_imports(code: &str) -> Vec<String> {
    let mut imports = Vec::new();

    for line in code.lines() {
        let trimmed = line.trim();

        if let Some(caps) = IMPORT_RE.captures(trimmed) {
            if let Some(pkg) = caps.get(1) {
                imports.push(pkg.as_str().to_string());
            }
        }

        if let Some(caps) = FROM_IMPORT_RE.captures(trimmed) {
            if let Some(pkg) = caps.get(1) {
                imports.push(pkg.as_str().to_string());
            }
        }
    }

    // Remove duplicates
    imports.sort();
    imports.dedup();
    imports
}

/// Check if a package is in Python's standard library
pub fn is_stdlib(package: &str) -> bool {
    // Common Python 3 standard library modules
    const STDLIB_MODULES: &[&str] = &[
        "abc",
        "aifc",
        "argparse",
        "array",
        "ast",
        "asynchat",
        "asyncio",
        "asyncore",
        "atexit",
        "audioop",
        "base64",
        "bdb",
        "binascii",
        "binhex",
        "bisect",
        "builtins",
        "bz2",
        "calendar",
        "cgi",
        "cgitb",
        "chunk",
        "cmath",
        "cmd",
        "code",
        "codecs",
        "codeop",
        "collections",
        "colorsys",
        "compileall",
        "concurrent",
        "configparser",
        "contextlib",
        "contextvars",
        "copy",
        "copyreg",
        "crypt",
        "csv",
        "ctypes",
        "curses",
        "dataclasses",
        "datetime",
        "dbm",
        "decimal",
        "difflib",
        "dis",
        "distutils",
        "doctest",
        "email",
        "encodings",
        "enum",
        "errno",
        "faulthandler",
        "fcntl",
        "filecmp",
        "fileinput",
        "fnmatch",
        "fractions",
        "ftplib",
        "functools",
        "gc",
        "getopt",
        "getpass",
        "gettext",
        "glob",
        "graphlib",
        "grp",
        "gzip",
        "hashlib",
        "heapq",
        "hmac",
        "html",
        "http",
        "idlelib",
        "imaplib",
        "imghdr",
        "imp",
        "importlib",
        "inspect",
        "io",
        "ipaddress",
        "itertools",
        "json",
        "keyword",
        "lib2to3",
        "linecache",
        "locale",
        "logging",
        "lzma",
        "mailbox",
        "mailcap",
        "marshal",
        "math",
        "mimetypes",
        "mmap",
        "modulefinder",
        "msilib",
        "msvcrt",
        "multiprocessing",
        "netrc",
        "nis",
        "nntplib",
        "numbers",
        "operator",
        "optparse",
        "os",
        "ossaudiodev",
        "parser",
        "pathlib",
        "pdb",
        "pickle",
        "pickletools",
        "pipes",
        "pkgutil",
        "platform",
        "plistlib",
        "poplib",
        "posix",
        "posixpath",
        "pprint",
        "profile",
        "pstats",
        "pty",
        "pwd",
        "py_compile",
        "pyclbr",
        "pydoc",
        "queue",
        "quopri",
        "random",
        "re",
        "readline",
        "reprlib",
        "resource",
        "rlcompleter",
        "runpy",
        "sched",
        "secrets",
        "select",
        "selectors",
        "shelve",
        "shlex",
        "shutil",
        "signal",
        "site",
        "smtpd",
        "smtplib",
        "sndhdr",
        "socket",
        "socketserver",
        "spwd",
        "sqlite3",
        "ssl",
        "stat",
        "statistics",
        "string",
        "stringprep",
        "struct",
        "subprocess",
        "sunau",
        "symbol",
        "symtable",
        "sys",
        "sysconfig",
        "syslog",
        "tabnanny",
        "tarfile",
        "telnetlib",
        "tempfile",
        "termios",
        "test",
        "textwrap",
        "threading",
        "time",
        "timeit",
        "tkinter",
        "token",
        "tokenize",
        "tomllib",
        "trace",
        "traceback",
        "tracemalloc",
        "tty",
        "turtle",
        "turtledemo",
        "types",
        "typing",
        "unicodedata",
        "unittest",
        "urllib",
        "uu",
        "uuid",
        "venv",
        "warnings",
        "wave",
        "weakref",
        "webbrowser",
        "winreg",
        "winsound",
        "wsgiref",
        "xdrlib",
        "xml",
        "xmlrpc",
        "zipapp",
        "zipfile",
        "zipimport",
        "zlib",
        "_thread",
    ];

    STDLIB_MODULES.contains(&package)
}

// ── Multi-file project blueprint ────────────────────────────────────────

/// A single file in a generated project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectFile {
    pub path: String,
    pub content: String,
}

/// A complete project structure generated by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectBlueprint {
    pub project_name: String,
    pub description: String,
    pub files: Vec<ProjectFile>,
}

/// Regex to extract JSON from a ```json code fence.
static JSON_BLOCK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"```\s*json\s*\n([\s\S]*?)\s*```").unwrap());

/// Parse LLM response into a `ProjectBlueprint`.
///
/// Tries direct JSON parsing first, then falls back to extracting JSON from
/// a markdown code fence. Validates that all file paths are safe (no `..`, no absolute paths).
pub fn parse_project_blueprint(response: &str) -> Result<ProjectBlueprint> {
    let json_str = if let Ok(bp) = serde_json::from_str::<ProjectBlueprint>(response.trim()) {
        return validate_blueprint(bp);
    } else if let Some(capture) = JSON_BLOCK_RE.captures(response) {
        capture.get(1).map(|m| m.as_str().trim().to_string())
    } else {
        None
    };

    let json_str = json_str.ok_or_else(|| anyhow!(
        "Could not find a valid JSON project blueprint in the LLM response. Expected a JSON object with project_name, description, and files array."
    ))?;

    let bp: ProjectBlueprint = serde_json::from_str(&json_str)
        .with_context(|| "Failed to parse project blueprint JSON")?;

    validate_blueprint(bp)
}

/// Validate that all file paths in the blueprint are safe.
fn validate_blueprint(bp: ProjectBlueprint) -> Result<ProjectBlueprint> {
    for file in &bp.files {
        if file.path.contains("..") {
            return Err(anyhow!(
                "Unsafe path detected: '{}' contains '..'",
                file.path
            ));
        }
        if file.path.starts_with('/') {
            return Err(anyhow!(
                "Unsafe path detected: '{}' is an absolute path",
                file.path
            ));
        }
    }
    if bp.project_name.is_empty() {
        return Err(anyhow!("Project name is empty"));
    }
    if bp.files.is_empty() {
        return Err(anyhow!("Project has no files"));
    }
    Ok(bp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_python_code_with_markdown() {
        let input = "```python\nprint('hello')\n```";
        let result = extract_python_code(input);
        assert_eq!(result, "print('hello')");
    }

    #[test]
    fn test_extract_python_code_without_language() {
        let input = "```\nprint('hello')\n```";
        let result = extract_python_code(input);
        assert_eq!(result, "print('hello')");
    }

    #[test]
    fn test_extract_python_code_plain_text() {
        let input = "print('hello')";
        let result = extract_python_code(input);
        assert_eq!(result, "print('hello')");
    }

    #[test]
    fn test_extract_python_code_multiline() {
        let input = "```python\ndef hello():\n    print('world')\n\nhello()\n```";
        let result = extract_python_code(input);
        assert_eq!(result, "def hello():\n    print('world')\n\nhello()");
    }

    #[test]
    fn test_extract_python_code_with_markdown_explanation() {
        let input = "### Step 2: Create the Game Code\n\nHere is the complete code for the Flappy Bird game:";
        let result = extract_python_code(input);
        assert!(result.contains("No Python code was generated"));
    }

    #[test]
    fn test_extract_python_code_mixed_markdown_and_code() {
        let input = "Here is your code:\n```python\nprint('hello')\n```\nThis code prints hello.";
        let result = extract_python_code(input);
        assert_eq!(result, "print('hello')");
    }

    #[test]
    fn test_extract_python_code_multiple_blocks() {
        let input = "```python\nimport pygame\n```\n\nSome text here\n\n```python\nscreen = pygame.display.set_mode((800, 600))\n```";
        let result = extract_python_code(input);
        // Should extract and concatenate both code blocks
        assert!(result.contains("import pygame"));
        assert!(result.contains("pygame.display"));
    }

    #[test]
    fn test_is_just_markdown_text() {
        let markdown = "### Step 1\nHere is the code:";
        assert!(is_just_markdown_text(markdown));

        let code = "import pygame\npygame.init()";
        assert!(!is_just_markdown_text(code));
    }

    #[test]
    fn test_extract_imports_simple() {
        let code = "import os\nimport sys";
        let result = extract_imports(code);
        assert_eq!(result, vec!["os", "sys"]);
    }

    #[test]
    fn test_extract_imports_from() {
        let code = "from pathlib import Path\nfrom os import path";
        let result = extract_imports(code);
        assert_eq!(result, vec!["os", "pathlib"]);
    }

    #[test]
    fn test_extract_imports_mixed() {
        let code = "import numpy\nfrom pandas import DataFrame\nimport requests";
        let result = extract_imports(code);
        assert_eq!(result, vec!["numpy", "pandas", "requests"]);
    }

    #[test]
    fn test_extract_imports_duplicates() {
        let code = "import os\nfrom os import path\nimport os";
        let result = extract_imports(code);
        assert_eq!(result, vec!["os"]);
    }

    #[test]
    fn test_extract_imports_with_comments() {
        let code = "# import fake\nimport real\n# from fake import test";
        let result = extract_imports(code);
        assert_eq!(result, vec!["real"]);
    }

    #[test]
    fn test_is_stdlib_standard_modules() {
        assert!(is_stdlib("os"));
        assert!(is_stdlib("sys"));
        assert!(is_stdlib("json"));
        assert!(is_stdlib("datetime"));
        assert!(is_stdlib("pathlib"));
    }

    #[test]
    fn test_is_stdlib_third_party() {
        assert!(!is_stdlib("numpy"));
        assert!(!is_stdlib("pandas"));
        assert!(!is_stdlib("requests"));
        assert!(!is_stdlib("flask"));
        assert!(!is_stdlib("django"));
    }

    #[test]
    fn test_ensure_dir_creates_new() {
        use std::path::PathBuf;
        let temp_dir = PathBuf::from("test_temp_dir_unique_12345");

        // Clean up if exists
        let _ = fs::remove_dir_all(&temp_dir);

        // Test creation
        let result = ensure_dir(&temp_dir);
        assert!(result.is_ok());
        assert!(temp_dir.exists());

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_ensure_dir_existing() {
        use std::path::PathBuf;
        let temp_dir = PathBuf::from("test_temp_dir_existing_12345");

        // Create directory first
        let _ = fs::create_dir_all(&temp_dir);

        // Test with existing directory
        let result = ensure_dir(&temp_dir);
        assert!(result.is_ok());
        assert!(temp_dir.exists());

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_find_char_boundary_ascii() {
        let s = "Hello, world!";
        assert_eq!(find_char_boundary(s, 5), 5);
        assert_eq!(find_char_boundary(s, 100), s.len());
        assert_eq!(find_char_boundary(s, 0), 0);
    }

    #[test]
    fn test_find_char_boundary_multibyte() {
        let s = "Héllo wörld"; // é is 2 bytes, ö is 2 bytes
                               // 'H' = 1 byte, 'é' = 2 bytes (bytes 1..3)
        assert_eq!(find_char_boundary(s, 2), 1); // mid-'é', snaps back to 1
        assert_eq!(find_char_boundary(s, 3), 3); // after 'é'
    }

    #[test]
    fn test_find_char_boundary_emoji() {
        let s = "Hi 👋 there";
        // 'H'=0, 'i'=1, ' '=2, '👋'=3..7
        assert_eq!(find_char_boundary(s, 4), 3); // mid-emoji, snaps back
        assert_eq!(find_char_boundary(s, 7), 7); // after emoji
    }

    // ── Project blueprint tests ─────────────────────────────────────────

    #[test]
    fn test_parse_project_blueprint_direct_json() {
        let json = r#"{
            "project_name": "my_project",
            "description": "A test project",
            "files": [
                {"path": "main.py", "content": "print('hello')"},
                {"path": "requirements.txt", "content": "requests"}
            ]
        }"#;
        let bp = parse_project_blueprint(json).unwrap();
        assert_eq!(bp.project_name, "my_project");
        assert_eq!(bp.files.len(), 2);
        assert_eq!(bp.files[0].path, "main.py");
    }

    #[test]
    fn test_parse_project_blueprint_from_code_fence() {
        let response = "Here is your project:\n```json\n{\"project_name\": \"test\", \"description\": \"desc\", \"files\": [{\"path\": \"main.py\", \"content\": \"print(1)\"}]}\n```";
        let bp = parse_project_blueprint(response).unwrap();
        assert_eq!(bp.project_name, "test");
        assert_eq!(bp.files.len(), 1);
    }

    #[test]
    fn test_parse_project_blueprint_rejects_dotdot() {
        let json = r#"{"project_name": "evil", "description": "d", "files": [{"path": "../etc/passwd", "content": "bad"}]}"#;
        assert!(parse_project_blueprint(json).is_err());
    }

    #[test]
    fn test_parse_project_blueprint_rejects_absolute_path() {
        let json = r#"{"project_name": "evil", "description": "d", "files": [{"path": "/etc/passwd", "content": "bad"}]}"#;
        assert!(parse_project_blueprint(json).is_err());
    }

    #[test]
    fn test_parse_project_blueprint_rejects_empty_name() {
        let json = r#"{"project_name": "", "description": "d", "files": [{"path": "main.py", "content": "ok"}]}"#;
        assert!(parse_project_blueprint(json).is_err());
    }

    #[test]
    fn test_parse_project_blueprint_rejects_no_files() {
        let json = r#"{"project_name": "empty", "description": "d", "files": []}"#;
        assert!(parse_project_blueprint(json).is_err());
    }
}
