# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Python Maker Bot (v0.2.1) is an AI-powered Python code generator written in Rust. It provides an interactive REPL that uses HuggingFace's Qwen2.5-Coder-7B-Instruct model to generate, refine, and execute Python code. The Rust source lives in `project_code/`.

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

## Architecture

```
project_code/src/
├── main.rs          # Entry point; loads .env, launches REPL via tokio async runtime
├── api.rs           # HuggingFace Inference API client (model config, system prompt, multi-turn chat)
├── interface.rs     # Interactive REPL loop and all slash commands (/help, /save, /refine, /run, etc.)
├── python_exec.rs   # Python execution engine with two modes: Captured (piped I/O) and Interactive (inherit I/O for pygame/tkinter/input)
├── utils.rs         # Code extraction from markdown responses, import parsing, stdlib detection
└── logger.rs        # Session file logging and in-memory metrics tracking
```

**Data flow:** User input → `interface` dispatches commands or sends prompt → `api` calls HuggingFace with full conversation history → `utils` extracts Python from markdown response → user confirms execution → `python_exec` detects dependencies, installs via pip, chooses execution mode, runs script → `logger` records results.

**Key design decisions:**
- Multi-turn conversation history (`Vec<Message>`) enables the `/refine` command for iterative code improvement
- Code extraction uses a 4-layer fallback: complete markdown blocks → truncated blocks → cleaned markdown → error message
- Interactive mode auto-detected by scanning for `pygame`, `input(`, `tkinter`, `turtle`, `curses`, `plt.show`, `cv2.imshow`
- Python interpreter resolution tries `python3` first, falls back to `python`
- API system prompt has extensive game development guidelines (Flappy Bird, Snake, Pong physics/controls)

## File Locations

- Generated scripts: `project_code/generated/script_YYYYMMDD_HHMMSS.py`
- Session logs: `project_code/logs/session_YYYYMMDD_HHMMSS.log`
- Both directories are gitignored

## Dependencies

Core: `tokio` (async), `reqwest` (HTTP/rustls-tls), `serde`/`serde_json` (serialization), `anyhow` (errors), `dotenvy` (.env), `chrono` (timestamps), `regex` (code extraction), `colored` (terminal UI), `rand`. Dev: `mockito` (HTTP mocking).
