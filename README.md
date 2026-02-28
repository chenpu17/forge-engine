# Forge Engine

Modular AI agent engine in Rust. Provides a multi-provider LLM agent loop with tool execution, session management, and SDK bindings for Node.js (N-API) and Python (PyO3).

## Architecture

```
Bindings:   forge-napi, forge-python
SDK:        forge-sdk
Agent:      forge-agent, forge-workflow
Services:   forge-tools, forge-tools-coding, forge-llm, forge-session,
            forge-lsp, forge-mcp, forge-memory, forge-prompt
Foundation: forge-domain, forge-config, forge-infra
```

16 crates, 918 tests, zero `unsafe` code.

## Features

- Multi-provider LLM support — Anthropic, OpenAI, Gemini, Ollama, and OpenAI-compatible endpoints
- Streaming agent loop with tool execution, reflection, and error recovery
- Built-in tools: file read/write/edit, bash, glob, grep, notebook, and more
- MCP (Model Context Protocol) server integration
- Session management with context compression
- Memory system with indexing and persistence
- Prompt templates and persona system
- Workflow engine (node graph + state machine)
- Path security: traversal prevention, sensitive file blocking, working directory enforcement
- Sandbox support (Unix)
- i18n (English / Chinese)
- OpenTelemetry observability

## Quick Start

### Rust

```rust
use forge_sdk::{ForgeSDK, ForgeSDKBuilder};

let sdk = ForgeSDKBuilder::new()
    .working_dir(".")
    .provider_name("anthropic")
    .model("claude-sonnet-4-20250514")
    .max_tokens(8192)
    .with_builtin_tools()
    .build()?;
```

### Node.js (N-API)

```javascript
const { ForgeConfig, ForgeSDK } = require('@forge/sdk');

const config = new ForgeConfig();
config.setProvider('anthropic');
config.setModel('claude-sonnet-4-20250514');

const sdk = new ForgeSDK(config);
await sdk.init();
```

### Python (PyO3)

```python
from forge_python import ForgeConfig, ForgeSDK

config = ForgeConfig(provider="anthropic", model="claude-sonnet-4-20250514")
sdk = ForgeSDK(config)
sdk.init()
```

## Build & Test

```bash
# Prerequisites: Rust 1.75+
cargo build
cargo test
cargo fmt --check
cargo clippy --workspace --all-targets
```

## Configuration

API key via environment variable:

```bash
export FORGE_LLM_API_KEY="sk-..."
# or provider-specific:
export ANTHROPIC_API_KEY="sk-ant-..."
export OPENAI_API_KEY="sk-..."
```

See `configs/default.toml` for full configuration reference.

## Crate Overview

| Crate | Purpose |
|-------|---------|
| `forge-domain` | Shared types: AgentEvent, ToolCall, ToolResult, ConfirmationLevel |
| `forge-config` | Configuration loading (env > project > global > default) |
| `forge-infra` | Logging, HTTP client, sandbox, i18n, keychain, storage |
| `forge-prompt` | Prompt templates, persona system |
| `forge-memory` | Memory loader, writer, indexing |
| `forge-llm` | LLM provider adapters, auth rotation, streaming |
| `forge-lsp` | LSP client management |
| `forge-mcp` | MCP server lifecycle and tool discovery |
| `forge-session` | Session management, context compression |
| `forge-tools` | Tool trait, registry, built-in tools, permission policy |
| `forge-tools-coding` | Coding-specific tools (optional plugin) |
| `forge-workflow` | Workflow engine (node graph, state machine, persistence) |
| `forge-agent` | Core agent loop, reflector, sub-agents, checkpoints |
| `forge-sdk` | Public SDK: ForgeSDK, ForgeSDKBuilder, AgentEvent stream |
| `forge-napi` | Node.js N-API bindings |
| `forge-python` | Python PyO3 bindings |

## Documentation

- [Architecture](docs/architecture.md)
- [SDK Guide](docs/sdk-guide.md)
- [Persona Guide](docs/persona-guide.md)
- [Plugin Guide](docs/plugin-guide.md)
- [CI & Release](docs/ci-release.md)

## License

[MIT](LICENSE)
