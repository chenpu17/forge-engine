# SDK Guide

## Quick Start

```rust
use forge_sdk::{ForgeSDK, ForgeSDKBuilder, AgentEvent, AgentEventExt};

let sdk = ForgeSDKBuilder::new()
    .working_dir(".")
    .provider_name("anthropic")
    .model("claude-sonnet-4-20250514")
    .with_builtin_tools()
    .build()?;

let config = sdk.config().await;
println!("Model: {}", config.llm.model);
```

## ForgeSDKBuilder

Chain configuration before building:

```rust
let sdk = ForgeSDKBuilder::new()
    .working_dir("/path/to/project")
    .provider_name("openai")
    .model("gpt-4o")
    .max_tokens(8192)
    .with_builtin_tools()
    .build()?;
```

## AgentEvent

Events emitted during agent execution:

| Event | Description |
|-------|-------------|
| TextDelta | Streaming text from LLM |
| ToolCallStart | Tool invocation started |
| ToolResult | Tool execution completed |
| TokenUsage | Token consumption update |
| Done | Agent finished |
| Error | Error occurred |
| ConfirmationRequired | User confirmation needed |

## Node.js (N-API)

```javascript
const { ForgeConfig, ForgeSDK } = require('@forge/sdk');

const config = new ForgeConfig();
config.setProvider('anthropic');
config.setModel('claude-sonnet-4-20250514');

const sdk = new ForgeSDK(config);
await sdk.init();
```

## Python (PyO3)

```python
from forge_python import ForgeConfig, ForgeSDK

config = ForgeConfig(provider="anthropic", model="claude-sonnet-4-20250514")
sdk = ForgeSDK(config)
sdk.init()
```
