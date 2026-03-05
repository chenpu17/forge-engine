# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Forge Engine — modular AI agent engine in Rust. 16 crates, SDK bindings for Node.js (N-API) and Python (PyO3).

## Architecture

```
Bindings:   forge-napi, forge-python
SDK:        forge-sdk
Agent:      forge-agent, forge-workflow
Services:   forge-tools, forge-tools-coding, forge-llm, forge-session, forge-lsp, forge-mcp, forge-memory, forge-prompt
Foundation: forge-domain, forge-config, forge-infra
```

**Dependency flow**: Foundation → Services → Agent → SDK → Bindings

**Key crates**:
- `forge-domain`: Shared types (AgentEvent, ToolCall, ToolResult)
- `forge-agent`: Core agent loop with streaming, reflection, error recovery
- `forge-sdk`: Public API (ForgeSDK, ForgeSDKBuilder)
- `forge-llm`: Multi-provider support (Anthropic, OpenAI, Gemini, Ollama)
- `forge-tools`: Tool registry, built-in tools, permission policy

## Commands

```bash
cargo build                          # Build all
cargo test                           # Test all
cargo test -p forge-sdk              # Test specific crate
cargo test -p forge-agent -- core_loop  # Test specific module
cargo fmt --check                    # Check formatting
cargo clippy --workspace --all-targets  # Full lint check
```

## Configuration

API keys via environment variables:
```bash
export FORGE_LLM_API_KEY="sk-..."
# or provider-specific:
export ANTHROPIC_API_KEY="sk-ant-..."
export OPENAI_API_KEY="sk-..."
```

Config hierarchy: env > project (`configs/`) > global (`~/.forge/`) > default

## Conventions

- `thiserror` for errors, no `unwrap()` (use `expect()` in tests)
- `tokio` async runtime, `async_trait` for async traits
- Dependencies declared in root `Cargo.toml` `[workspace.dependencies]`
- Public APIs require `///` doc comments
- Clippy: pedantic + nursery (warn), `unsafe_code = "deny"`
- Commit format: `type(scope): description`
