# CLAUDE.md — Developer Guide for Python Maker Bot

## Build & Run

```bash
cargo build --release      # Release build
cargo run                  # Run the REPL (debug mode)
cargo run --release        # Run the REPL (release mode)
```

## Testing

```bash
cargo test                        # Run all tests (104+ unit + integration)
cargo test test_name              # Run a specific test
cargo test -- --nocapture         # Show println! output during tests
cargo test --test integration_tests  # Run only integration tests
```

## Code Quality

```bash
cargo fmt -- --check       # Check formatting
cargo fmt                  # Auto-format
cargo clippy -- -D warnings  # Lint (zero warnings policy)
```

## Docker Sandbox

```bash
docker build -t python-sandbox .   # Build the sandbox image
# Then set use_docker = true in pymakebot.toml
```

## Architecture

| Module | Responsibility |
|--------|----------------|
| `src/main.rs` | Entry point — delegates to `lib.rs::run()` |
| `src/lib.rs` | Library entrypoint, re-exports `AppConfig`, `CodeExecutor`, `ExecutionMode` |
| `src/config.rs` | TOML config loading with chain: `./pymakebot.toml` → `~/pymakebot.toml` → defaults |
| `src/api.rs` | Multi-provider LLM client (HuggingFace, Ollama, OpenAI-compatible), retry with backoff, embeddings |
| `src/interface.rs` | Interactive REPL with slash commands, tab-completion, spinner, syntax highlighting |
| `src/python_exec.rs` | Code execution engine: Docker sandbox, venv isolation, ruff linting, bandit security scanning |
| `src/utils.rs` | Code extraction from markdown, import parsing, UTF-8 safe slicing |
| `src/logger.rs` | Session logging to timestamped files, `SessionMetrics` tracking |
| `src/rag.rs` | RAG store: text chunking, embedding generation, cosine similarity retrieval |
| `src/dashboard/` | Axum web dashboard with REST API, HTMX partials, WebSocket real-time logs |

## Key Patterns & Conventions

- **Error handling**: `anyhow::Result<T>` with `.context()` throughout
- **Regex caching**: `std::sync::LazyLock<Regex>` in `utils.rs` — compiled once, reused
- **Shared state**: `Arc<DashboardState>` with `RwLock` fields for concurrent REPL + dashboard access
- **Templates**: Askama compile-time templates in `templates/`
- **Frontend**: HTMX for dynamic partials, Tailwind CSS (dark theme), highlight.js, WebSocket for logs
- **Provider abstraction**: `Provider` enum in `api.rs` with `from_config()`, `resolve_api_url()`, `auth_headers()`
- **Broadcast**: `tokio::sync::broadcast` channels for real-time WebSocket events

## Configuration

- **Config file**: `pymakebot.toml` (local dir → home dir → defaults)
- **Environment vars**: `HF_TOKEN` (required for HuggingFace), `LLM_API_KEY` (optional, for OpenAI-compatible)
- **`.env` file**: Loaded automatically via `dotenvy`

## Important Notes

- The `.gitignore` excludes `*.py` files (generated scripts) and `generated/` / `logs/` directories
- All providers use the OpenAI chat completions format (`stream: false` by default)
- Docker sandbox uses `--network none` for isolation and mounts scripts read-only
- Tests that modify environment variables are not thread-safe — run with `cargo test -- --test-threads=1` if flaky
