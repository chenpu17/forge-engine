# CLAUDE.md

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

## Commands

```bash
cargo build                          # Build all
cargo test                           # Test all
cargo test -p forge-sdk              # Test specific crate
cargo fmt && cargo clippy            # Format + lint
```

## Conventions

- `thiserror` for errors, no `unwrap()` (use `expect()` in tests)
- `tokio` async runtime, `async_trait` for async traits
- Dependencies declared in root `Cargo.toml` `[workspace.dependencies]`
- Public APIs require `///` doc comments
- Clippy: pedantic + nursery (warn), `unsafe_code = "deny"`
- Commit format: `type(scope): description`
