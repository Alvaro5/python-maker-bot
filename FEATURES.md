# Suggested Improvements to "Engineer" the Project
To make this stand out for a senior-year portfolio, consider moving from a "functional tool" to a "production-grade system":

1. ~~Security & Sandboxing (Highest Priority)~~ ✅ **DONE**
    ~~Currently, the tool executes AI-generated code directly on the host machine. This is a major security risk.~~

    Implemented in `python_exec.rs` via `use_docker = true` in `pymakebot.toml`. Scripts run inside an isolated `python-sandbox` Docker container with:
    - No network access (`--network none`)
    - Read-only script mount
    - Non-root `sandboxuser`
    - Dependency installation via `docker commit`
    - Timeout support and graceful fallback to host execution

    Build the image: `docker build -t python-sandbox .`

2. ~~Virtual Environment Isolation (host & Docker)~~ ✅ **DONE**
    ~~The project currently uses the system pip to install dependencies.~~

    Implemented in `python_exec.rs` via `use_venv = true` (default) in `pymakebot.toml`. Each script execution runs inside a temporary Python virtual environment:
    - **Host mode**: creates a temp venv in the OS temp directory, installs deps into it, runs the script with the venv's Python, then cleans up automatically.
    - **Docker mode**: creates a venv inside the ephemeral container at execution time (via `bash -c` entrypoint), installs deps and runs the script — all in a single `docker run --rm`. No image mutation (`docker commit`) needed.
    - Configurable: set `use_venv = false` in `pymakebot.toml` to revert to system-wide pip behavior.

3. ~~Versatile Model Support (Cloud + Local LLMs)~~ ✅ **DONE**
    ~~You currently rely on the HuggingFace API.~~

    Implemented in `api.rs` via a `Provider` enum (`HuggingFace`, `Ollama`, `OpenAiCompatible`). Configured with `provider` field in `pymakebot.toml`:
    - **HuggingFace** (default): cloud API, requires `HF_TOKEN` in `.env`.
    - **Ollama**: local inference at `http://localhost:11434/v1/chat/completions`, no API key required (optional `LLM_API_KEY` for proxy setups).
    - **OpenAI-compatible**: any endpoint following the OpenAI chat completions spec, with optional `LLM_API_KEY` auth.
    - Auto URL resolution: when switching provider with the default HF URL still set, the provider's default URL is used automatically.
    - New `/provider` REPL command shows active provider, model, and resolved API URL.
    - All providers use OpenAI-compatible chat completions format with `stream: false`.

4. ~~Static Analysis (Linting)~~ ✅ **DONE**
    ~~You currently use py_compile to check syntax.~~

    Implemented in `python_exec.rs` via `ruff check` integration. Configured with `use_linting = true` (default) in `pymakebot.toml`:
    - Runs `ruff check --output-format=concise --no-fix` after syntax check passes.
    - Classifies diagnostics as errors (E/F rules) or warnings (W/C/etc. rules).
    - On lint errors: offers auto-refine via LLM to fix the issues.
    - On warnings only: displays them but proceeds to execution.
    - New `/lint` REPL command to lint code on demand.
    - Graceful degradation: if `ruff` is not installed, linting is skipped with a helpful install message.
    - Configurable: set `use_linting = false` to disable.

5. ~~Static Security Analysis (The "Pre-Flight" Check)~~ ✅ **DONE**
    ~~Before the code ever reaches the Docker container, you should "inspect" it.~~

    Implemented in `python_exec.rs` via `bandit` integration. Configured with `use_security_check = true` (default) in `pymakebot.toml`:
    - Runs `bandit -f json -q` after syntax and lint checks pass, before execution.
    - Parses JSON output into structured diagnostics with severity (LOW/MEDIUM/HIGH) and confidence.
    - On HIGH severity findings: prompts the user before proceeding with execution.
    - On MEDIUM/LOW findings: displays them but does not block execution.
    - New `/security` REPL command to scan code on demand.
    - Graceful degradation: if `bandit` is not installed, security scanning is skipped with a helpful install message.
    - Configurable: set `use_security_check = false` to disable.

6. ~~Real-Time Web Dashboard (Axum + HTML/HTMX)~~ ✅ **DONE**
    ~~CLI tools are powerful but not user-friendly. A visual dashboard unlocks non-technical users and recruiters.~~

    Implemented in `src/dashboard/` module via `enable_dashboard = true` in `pymakebot.toml`. Provides a local web interface at `http://localhost:3000` (configurable port) running alongside the CLI REPL:
    - **Backend**: Axum web framework serving REST API endpoints and HTML pages:
      - `GET /` — main dashboard page with script history, prompt input, code viewer, and real-time logs
      - `GET /api/history` — JSON list of generated scripts with timestamps
      - `POST /api/generate` — accept a prompt, call the LLM, return generated code
      - `GET /api/stats` — session metrics (requests, successes, failures, success rate)
      - `GET /api/containers` — active Docker sandbox containers
      - `WS /api/logs` — WebSocket endpoint for real-time execution log streaming
    - **Frontend**: HTML/HTMX interface with Tailwind CSS dark theme:
      - Left sidebar: clickable script history (auto-refreshed via HTMX)
      - Center: prompt form with code generation, syntax-highlighted code viewer (highlight.js)
      - Bottom: real-time execution log panel (WebSocket-driven)
      - Right sidebar: session stats and Docker container status (HTMX-polled)
    - **Concurrency**: `tokio::sync::broadcast` channels stream execution events to WebSocket clients
    - **Shared State**: `Arc<DashboardState>` with `RwLock` for metrics, conversation history, and generated code — shared between REPL and dashboard
    - **Configuration**: `enable_dashboard = true` and `dashboard_port = 3000` in `pymakebot.toml`
    - New `/dashboard` REPL command shows the dashboard URL

7. ~~"Chat with Data" (Local RAG)~~ ✅ **DONE**
    ~~Currently, the bot generates code based only on LLM training data.~~

    Implemented in `src/rag.rs` with the `/context <file_path>` REPL command. Supports TXT, MD, and CSV files:
    - **Text chunking** with configurable chunk size and 10% overlap
    - **Embedding generation** via provider-specific APIs (HuggingFace, Ollama, OpenAI-compatible)
    - **In-memory vector store** with cosine similarity retrieval
    - **Automatic RAG injection** into prompts when `enable_rag = true`
    - Configurable: `enable_rag`, `rag_embedding_model`, `rag_chunk_size` in `pymakebot.toml`

8. Multi-File Project Generation (Scaffolding)
    Today, the bot returns single scripts. Professional engineering requires generating complete project structures.

    Feature: Instead of a single code block, the bot generates a folder structure with main files, dependencies, README, and utils. Output is written to a `generated/<project_name>/` directory.

    Why it impresses recruiters: It transforms the tool from a "code snippet generator" to a "productivity accelerator." Parsing and structuring complex LLM outputs (JSON/XML) demonstrates engineering maturity.

    Implementation Plan:
    - **Prompt Engineering**: Update the LLM system prompt to request JSON output following this schema:
      ```json
      {
        "project_name": "string",
        "description": "string",
        "files": [
          {"path": "main.py", "content": "..."},
          {"path": "requirements.txt", "content": "..."},
          {"path": "README.md", "content": "..."},
          {"path": "src/utils.py", "content": "..."}
        ]
      }
      ```
    - **Parsing**: Deserialize the JSON response into a Rust struct `ProjectBlueprint` using `serde_json`.
    - **File I/O**: Iterate through `files` and write each to `generated/<project_name>/<path>` using `std::fs::write`.
    - **Directory Creation**: Create parent directories as needed using `std::fs::create_dir_all`.
    - **Auto-Setup**: After generation, optionally auto-install dependencies:
      - For Python: run `pip install -r requirements.txt` in the generated directory (or in a venv).
      - Configurable: Add `auto_install_deps = true` option in `pymakebot.toml`.
    - **User Feedback**: Display the generated file tree and provide commands to open the folder or run the project.



9. ~~GitHub Actions CI/CD Pipeline~~ ✅ **DONE**
    Automated quality gates on every push and pull request:
    - `cargo fmt --check` — formatting verification
    - `cargo clippy -- -D warnings` — zero-warning lint policy
    - `cargo test` — full test suite execution
    - Uses Rust stable on ubuntu-latest with dependency caching

10. ~~Code Explanation Mode~~ ✅ **DONE**
    The bot can now explain generated code step-by-step via `/explain` REPL command and dashboard endpoint:
    - Dedicated explanation system prompt for educational breakdowns
    - Explains: overview, step-by-step walkthrough, key concepts, potential improvements
    - Dashboard "Explain" button for one-click explanations

11. ~~Conversation Persistence~~ ✅ **DONE**
    Sessions can be saved and loaded across restarts:
    - `/session save <name>` — serialize conversation history to JSON
    - `/session load <name>` — restore a previous session
    - `/session list` — browse saved sessions
    - Dashboard endpoints for session persistence

12. ~~Streaming LLM Responses~~ ✅ **DONE**
    Real-time token-by-token code generation:
    - REPL: characters appear as the model generates them
    - Dashboard: Server-Sent Events (SSE) for live streaming
    - Configurable via `use_streaming = true` in `pymakebot.toml`
    - Graceful fallback to non-streaming on error

13. ~~Multi-File Project Generation~~ ✅ **DONE**
    Generate entire project structures, not just single scripts:
    - `/project <prompt>` scaffolds a complete project directory
    - LLM outputs structured JSON with file paths and contents
    - Writes to `generated/<project_name>/` with proper directory structure
    - Dashboard endpoint for project generation

## Possible Optimizations

Optimization Tip: Creating a fresh venv inside the ephemeral container for every execution is secure but computationally expensive (slow).

Suggestion: For your portfolio demo, consider pre-baking common data science libraries (pandas, numpy, scikit-learn) into your Dockerfile so the bot doesn't have to install them every single time.

Optimization Tip: Ensure your prompt templates in api.rs are generic enough. Some local models (like Mistral) adhere strictly to specific chat templates ([INST]...[/INST]), whereas OpenAI is more flexible.