# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Python Maker Bot (v0.2.1) is an AI-powered Python code generator written in Rust. It provides an interactive REPL that uses HuggingFace's Qwen2.5-Coder-32B-Instruct model (configurable) to generate, refine, and execute Python code. The Rust source lives in `project_code/`.

## Build & Development Commands

All commands run from `project_code/`:

```bash
cargo build --release        # Build optimized binary
cargo run                    # Build and run (debug)
cargo run --release          # Build and run (release)
cargo test                   # Run all tests (unit + integration)
cargo test --lib             # Unit tests only
cargo test --test integration_tests  # Integration tests only
cargo test <test_name>       # Run a single test by name
cargo clippy                 # Lint
```

## Environment Setup

Requires a `HF_TOKEN` in a `.env` file at the project root or `project_code/` directory. See `.env.example` for the template.

## Configuration

Optional `pymakebot.toml` file (load chain: `./pymakebot.toml` → `~/pymakebot.toml` → defaults). All fields are optional:

```toml
model = "Qwen/Qwen2.5-Coder-32B-Instruct"
api_url = "https://router.huggingface.co/v1/chat/completions"
max_tokens = 16284
temperature = 0.2
execution_timeout_secs = 30
auto_install_deps = false
max_history_messages = 20
max_retries = 3
log_dir = "logs"
generated_dir = "generated"
```

## Architecture

```
project_code/src/
├── main.rs          # Entry point; loads .env, loads AppConfig, launches REPL via tokio async runtime
├── config.rs        # AppConfig struct with TOML deserialization and load chain (local → home → defaults)
├── api.rs           # HuggingFace Inference API client with retry + exponential backoff
├── interface.rs     # Interactive REPL loop, slash commands, syntax check, auto-refine, history trimming
├── python_exec.rs   # Python execution engine: write, syntax check, execute with timeout (Captured/Interactive modes)
├── utils.rs         # Code extraction (cached regexes via LazyLock), import parsing, stdlib detection, UTF-8 safe slicing
└── logger.rs        # Session file logging and in-memory metrics tracking
```

**Data flow:** User input → `interface` dispatches commands or sends prompt → `api` calls HuggingFace with full conversation history (with retry/backoff) → `utils` extracts Python from markdown response → script is written to disk → `python_exec` runs syntax check → on error, offers auto-refine via API → user confirms execution → `python_exec` detects dependencies, installs via pip, chooses execution mode, runs script with timeout → `logger` records results → conversation history is trimmed.

**Key design decisions:**
- `AppConfig` loaded from `pymakebot.toml` and threaded through `main` → `interface` → `api`
- Multi-turn conversation history (`Vec<Message>`) enables the `/refine` command; trimmed to `max_history_messages`
- API retry with exponential backoff (1s, 2s, 4s + jitter); retries on network errors, 429, 5xx; fails fast on 4xx client errors
- Code extraction uses cached `LazyLock<Regex>` statics (compiled once) with 4-layer fallback
- Syntax check via `python3 -m py_compile` before execution, with auto-refine on failure
- Execution timeout via `wait-timeout` crate (Captured mode only; no timeout for Interactive mode)
- `CodeExecutionResult::is_success()` checks `exit_code == Some(0)` — no false positives
- `find_char_boundary()` for safe UTF-8 string slicing (prevents panics on multi-byte characters)
- Interactive mode auto-detected by scanning for `pygame`, `input(`, `tkinter`, `turtle`, `curses`, `plt.show`, `cv2.imshow`
- Python interpreter resolution tries `python3` first, falls back to `python`

## File Locations

- Generated scripts: `project_code/generated/script_YYYYMMDD_HHMMSS.py`
- Session logs: `project_code/logs/session_YYYYMMDD_HHMMSS.log`
- Config file: `./pymakebot.toml` or `~/pymakebot.toml`
- Both generated/ and logs/ directories are gitignored

## Dependencies

Core: `tokio` (async), `reqwest` (HTTP/rustls-tls), `serde`/`serde_json` (serialization), `anyhow` (errors), `dotenvy` (.env), `chrono` (timestamps), `regex` (code extraction), `colored` (terminal UI), `rand`, `toml` (config parsing), `dirs` (home directory), `wait-timeout` (execution timeout). Dev: `mockito` (HTTP mocking).
