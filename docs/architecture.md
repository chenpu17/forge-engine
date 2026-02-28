# Architecture

## Overview

Forge Engine is a modular AI agent engine built in Rust. It provides SDK bindings for Node.js (N-API) and Python (PyO3).

## Dependency Layers

```
Layer 4 (Bindings):   forge-napi, forge-python
Layer 3 (SDK):        forge-sdk
Layer 2 (Agent):      forge-agent, forge-workflow
Layer 1 (Services):   forge-tools, forge-tools-coding, forge-llm, forge-session,
                      forge-lsp, forge-mcp, forge-memory, forge-prompt
Layer 0 (Foundation): forge-domain, forge-config, forge-infra
```

## Data Flow

```
Frontend (TUI/Desktop/Web)
    │
    ▼
forge-sdk (ForgeSDK + ForgeSDKBuilder)
    │
    ▼
forge-agent (CoreAgent loop: gather context → LLM call → tool exec → reflect)
    │
    ├──► forge-llm (Provider adapters: Anthropic, OpenAI, Gemini, Ollama)
    ├──► forge-tools (Tool registry + builtin tools + script plugins)
    ├──► forge-session (Context window, history, compression)
    ├──► forge-mcp (MCP server connections)
    └──► forge-lsp (Language server integration)
```

## Crate Responsibilities

| Crate | Purpose |
|-------|---------|
| forge-domain | Shared types: AgentEvent, ToolCall, ToolResult, ConfirmationLevel |
| forge-config | Configuration loading (env > project > global > default) |
| forge-infra | Cross-cutting: logging, OTel, HTTP client, sandbox, i18n |
| forge-prompt | Prompt templates, persona system, PromptManager |
| forge-memory | Memory system (loader, writer, indexing) |
| forge-llm | LLM provider adapters, auth rotation, streaming |
| forge-lsp | LSP client management (optional, coding-specific) |
| forge-mcp | MCP server lifecycle and tool discovery |
| forge-session | Session management, context compression |
| forge-tools | Tool trait, registry, builtin tools, permission policy |
| forge-tools-coding | Coding-specific tools (optional plugin) |
| forge-workflow | Workflow engine (node graph, state machine, persistence) |
| forge-agent | Core agent loop, reflector, sub-agents, checkpoints |
| forge-sdk | Public SDK API: ForgeSDK, ForgeSDKBuilder, AgentEvent stream |
| forge-napi | Node.js N-API bindings |
| forge-python | Python PyO3 bindings |
