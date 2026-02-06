# 🤖 Python Maker Bot v0.2.1

**An AI-Powered Python Code Generator Built with Rust**

> 🎮 Now with Interactive Mode for games, GUIs, and programs requiring user input!

A Rust-based interactive shell that leverages AI language models to generate, refine, and execute Python code on demand. This agentic tool helps you quickly prototype Python scripts with conversational AI assistance.

---

## ✨ Features

### 🎯 Core Functionality
- **AI-Powered Code Generation**: Uses HuggingFace's Qwen2.5-Coder-32B-Instruct model for high-quality Python code (configurable)
- **Interactive REPL**: Easy-to-use command-line interface with helpful commands
- **Automatic Code Execution**: Run generated Python scripts directly from the shell
- **Smart Code Extraction**: Handles markdown-formatted responses and extracts clean Python code

### 🔄 Advanced Capabilities
- **Multi-Turn Refinement**: Maintain conversation history to iteratively improve code
- **Interactive Mode** 🎮: Automatically detects and runs interactive programs (pygame games, user input, GUIs)
- **Docker Sandbox** 🐳: Optionally execute AI-generated code inside an isolated Docker container for security
- **Syntax Check & Auto-Refine**: Validates code with `py_compile` before execution; offers to auto-fix syntax errors via AI
- **API Retry with Backoff**: Automatic retries with exponential backoff on network errors, rate limits, and server errors
- **Execution Timeout**: Configurable timeout kills runaway scripts (Captured mode only)
- **Conversation History Limit**: Automatically trims old messages to keep context manageable
- **Script Management**: List and re-run previously generated scripts anytime
- **Dependency Detection**: Automatically detects non-standard library imports
- **Auto-Installation**: Prompts to install required packages via pip (or auto-installs with config)
- **Session Logging**: All API calls and executions logged to timestamped files
- **Success Metrics**: Track and display success rates and session statistics

### 🎨 User Experience
- **Colored Output**: Syntax-highlighted code display with colorized terminal output
- **File Management**: Save generated code to files with `/save` command
- **History Tracking**: View conversation history with `/history`
- **Session Stats**: Monitor performance with `/stats`

---

## 🚀 Quick Start

### Prerequisites

- **Rust** (1.80+): [Install Rust](https://rustup.rs/) — requires `LazyLock` support
- **Python 3**: For executing generated scripts
- **HuggingFace Token**: Required for API access (free tier available)
- **Docker** (optional): For sandboxed script execution — [Install Docker](https://docs.docker.com/get-docker/)

### Installation

1. **Clone the repository**:
```bash
git clone https://github.com/Ali-Gatorre/Rust_project.git
cd Rust_project
```

2. **Set up HuggingFace Token**:
Create a `.env` file in the repository root:
```bash
echo "HF_TOKEN=your_huggingface_token_here" > .env
```

Get your token from [HuggingFace Settings](https://huggingface.co/settings/tokens)

3. **Build and run**:
```bash
cargo build --release
cargo run
```

4. **(Optional) Build the Docker sandbox image**:
```bash
docker build -t python-sandbox .
```
Then enable it in `pymakebot.toml`:
```toml
use_docker = true
```

---

## 📖 Usage Guide

### Interactive Commands

| Command | Description |
|---------|-------------|
| `/help` | Show all available commands |
| `/quit` or `/exit` | Exit the program |
| `/clear` | Clear conversation history |
| `/refine` | Refine the last generated code |
| `/save <filename>` | Save last code to a file |
| `/history` | Show conversation history |
| `/stats` | Display session statistics |
| `/list` | List all previously generated scripts |
| `/run <filename>` | Execute a previously generated script |

### Example Session

```
> Create a script that prints fibonacci numbers up to 100

----------- Generated Code -----------
# Fibonacci numbers up to 100
def fibonacci(limit):
    a, b = 0, 1
    while a < limit:
        print(a, end=' ')
        a, b = b, a + b
    print()

fibonacci(100)
-----------------------------------

Execute this script? (y/n) : y

--- Execution Result ---
STDOUT:
0 1 1 2 3 5 8 13 21 34 55 89
```

### Multi-Turn Refinement

```
> Create a simple calculator
[Code generated...]

> /refine
What would you like to change or add? Add division by zero handling
[Improved code generated...]
```

### Interactive Programs (NEW in v0.2!)

```
> Create a pygame game with a bouncing ball

⚠️  Detected non-standard dependencies: pygame
Install these dependencies? (y/n) : y
✓ Dependencies installed successfully

----------- Generated Code -----------
import pygame
# ... game code ...
-----------------------------------

Execute this script? (y/n) : y

🎮 Interactive mode detected (pygame/input/GUI)
   Running with inherited stdio for user interaction...

[Pygame window opens with bouncing ball animation]
```

See [INTERACTIVE_MODE.md](INTERACTIVE_MODE.md) for detailed documentation on running games, programs with user input, and GUI applications.

---

## 🏗️ Architecture

### Project Structure

```
.
├── src/
│   ├── main.rs          # Entry point; loads .env and config
│   ├── config.rs        # AppConfig with TOML deserialization
│   ├── api.rs           # HuggingFace API client with retry/backoff
│   ├── interface.rs     # Interactive REPL with syntax check and auto-refine
│   ├── python_exec.rs   # Python execution engine with timeout & Docker sandbox
│   ├── utils.rs         # Code extraction, import parsing, UTF-8 utils
│   └── logger.rs        # Logging and metrics
├── generated/           # Generated Python scripts
├── logs/                # Session logs
├── Cargo.toml           # Rust dependencies
├── Dockerfile           # Docker sandbox image definition
└── pymakebot.toml       # Optional configuration file
```

### Technology Stack

- **Language**: Rust 2021 Edition
- **AI Model**: Qwen/Qwen2.5-Coder-32B-Instruct (HuggingFace) — configurable
- **Key Dependencies**:
  - `reqwest`: HTTP client for API calls
  - `tokio`: Async runtime
  - `serde/serde_json`: JSON serialization
  - `toml/dirs`: Configuration file support
  - `wait-timeout`: Execution timeout
  - `colored`: Terminal color output
  - `regex`: Code extraction (cached with `LazyLock`)
  - `chrono`: Timestamps
  - `rand`: Retry jitter

---

## 🔧 Configuration

### Environment Variables

- `HF_TOKEN`: Your HuggingFace API token (required, via `.env` file)

### Configuration File (`pymakebot.toml`)

Create an optional `pymakebot.toml` in the project directory or your home directory. All fields are optional — missing fields use defaults:

```toml
# AI model settings
model = "Qwen/Qwen2.5-Coder-32B-Instruct"
api_url = "https://router.huggingface.co/v1/chat/completions"
max_tokens = 16284
temperature = 0.2

# Execution settings
execution_timeout_secs = 30    # Kill scripts after this many seconds (0 = no timeout)
auto_install_deps = false      # Auto-install detected dependencies without prompting
use_docker = false             # Run scripts inside Docker sandbox (requires: docker build -t python-sandbox .)

# API resilience
max_retries = 3                # Retry on network errors, 429, and 5xx responses

# History management
max_history_messages = 20      # Trim oldest messages when history exceeds this

# File locations
log_dir = "logs"
generated_dir = "generated"
```

**Load order**: `./pymakebot.toml` → `~/pymakebot.toml` → built-in defaults

---

## 📊 Logging and Metrics

### Session Logs

All sessions are logged to `logs/session_TIMESTAMP.log` with:
- API requests and responses
- Execution results
- Errors and warnings

### Metrics Tracked

- Total API requests
- Successful vs failed executions
- API errors
- Success rate percentage

View anytime with `/stats`

---

## 🛡️ Security Considerations

⚠️ **Important**: This tool executes AI-generated code automatically.

**Best Practices**:
1. **Enable Docker sandbox** (`use_docker = true`) to isolate generated code from your host
2. Review generated code before execution
3. Don't commit your `.env` file
4. Monitor API usage to avoid unexpected costs
5. Be cautious with file system operations in generated code

**Safety Features**:
- **Docker sandbox**: Runs scripts in an isolated container with no network access and read-only script mount
- Syntax check via `py_compile` before execution catches errors early
- Execution timeout prevents runaway scripts
- Dependency detection warns about non-standard imports before install
- Graceful fallback to host execution if Docker is unavailable

**Limitations**:
- Requires HuggingFace Pro for heavy usage (free tier has rate limits)
- Generated code quality depends on prompt clarity

---

## 🤝 Contributing

Contributions are welcome! Areas for improvement:

- [ ] Implement virtual environment isolation per script
- [ ] Support for additional AI models (OpenAI, Anthropic, etc.)
- [ ] Web UI using Tauri or similar
- [ ] Support for other programming languages

---

## 📚 Documentation

Complete guides available:

- **[DEMO_EXAMPLES.md](DEMO_EXAMPLES.md)** - Battle-tested examples perfect for demos and presentations
- **[DEFENSE_CHEATSHEET.md](DEFENSE_CHEATSHEET.md)** - Quick reference for project defense/presentations
- **[QUICK_REFERENCE.md](QUICK_REFERENCE.md)** - Command cheat sheet and feature overview
- **[INTERACTIVE_MODE.md](INTERACTIVE_MODE.md)** - Technical deep dive into interactive execution
- **[EXAMPLES.md](EXAMPLES.md)** - Usage examples and patterns
- **[FIX_SUMMARY.md](FIX_SUMMARY.md)** - Technical implementation details
- **[ARCHITECTURE_DIAGRAM.md](ARCHITECTURE_DIAGRAM.md)** - Visual system diagrams

---

## 📝 License

MIT License - see LICENSE file for details

---

## 🙏 Acknowledgments

- **HuggingFace** for providing the inference API
- **Qwen Team** for the excellent code generation model
- Rust community for amazing libraries

---

## 📞 Contact

- **GitHub**: [Ali-Gatorre](https://github.com/Ali-Gatorre)
- **Project**: [Rust_project](https://github.com/Ali-Gatorre/Rust_project)

---

## 🔄 Version History

### v0.2.2 (Current - February 2026)
- � **Docker Sandbox**: Execute AI-generated scripts inside an isolated Docker container (`use_docker = true`)
  - Network-isolated execution (`--network none`), read-only script mount
  - Dependency installation inside the container (persisted via `docker commit`)
  - Supports both Captured and Interactive execution modes
  - Graceful fallback to host execution when Docker is unavailable
- 🔧 **Configuration File**: `pymakebot.toml` support with load chain (local → home → defaults)
- 🔁 **API Retry**: Exponential backoff with jitter on network errors, 429, and 5xx
- ⏱️ **Execution Timeout**: Configurable timeout kills runaway scripts in Captured mode
- ✅ **Syntax Check**: Pre-execution validation via `py_compile` with auto-refine on errors
- 📏 **History Limit**: Automatic trimming of conversation history to configured max
- 🐛 **Bug Fixes**: UTF-8 safe string slicing, correct success detection (`exit_code == 0`), cached regex compilation
- 🧹 **Code Quality**: Zero clippy warnings, 68 tests (61 unit + 7 integration)

### v0.2.1 (December 2025)
- 🎮 **Interactive Mode**: Automatic detection for pygame, input(), tkinter, GUIs
- 📂 **Script Management**: `/list` and `/run` commands for previously generated scripts
- 🎯 **Enhanced AI Prompts**: Better code generation with self-contained scripts (no external files)
- 🔧 **Token Limit Increase**: 4096 tokens for complete game implementations
- 🧹 **Smart Code Extraction**: Multiple fallback strategies for markdown cleanup
- 📝 **Comprehensive Documentation**: 7 new documentation files

### v0.2.0 (December 2025)
- Multi-turn conversation support with history
- Dependency detection and auto-installation
- Colored terminal output with syntax highlighting
- Session logging and metrics tracking
- Enhanced command system (/save, /history, /stats, /refine)

### v0.1.0 (Initial)
- Basic code generation via HuggingFace API
- Simple Python script execution
- Core REPL interface

---

**Made with ❤️ by Alvaro Serero, Ali Dabale, Chloé Daunas and Ovia Chanemouganandam**
