# Plugin Guide

## Overview

Forge Engine supports two types of tool plugins:
1. **Rust crate plugins** (e.g., forge-tools-coding)
2. **Script plugins** (shell/Python/JS scripts in `.forge/tools/`)

## Script Plugins

Create a directory under `.forge/tools/<tool_name>/`:

```
.forge/tools/my_tool/
├── tool.json    # Manifest
└── tool.sh      # Executable
```

### tool.json

```json
{
  "name": "my_tool",
  "description": "What this tool does",
  "parameters": {
    "type": "object",
    "properties": {
      "input": { "type": "string", "description": "Input value" }
    },
    "required": ["input"]
  }
}
```

### tool.sh

```bash
#!/bin/bash
# Parameters are passed as JSON via stdin
INPUT=$(cat | jq -r '.input')
echo "Result: $INPUT"
```

Script plugins are auto-discovered. Project-level (`.forge/tools/`) takes priority over user-level (`~/.forge/tools/`).

## Rust Crate Plugins

See `crates/forge-tools-coding/` as a reference. Implement the `Tool` trait:

```rust
use forge_domain::tool::{Tool, ToolOutput, ConfirmationLevel};

pub struct MyTool;

#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &str { "my_tool" }
    fn description(&self) -> &str { "What this tool does" }
    fn parameters_schema(&self) -> serde_json::Value { /* JSON Schema */ }
    async fn execute(&self, params: serde_json::Value, ctx: &ToolContext) -> ToolOutput { /* ... */ }
    fn confirmation_level(&self) -> ConfirmationLevel { ConfirmationLevel::None }
}
```

Register via `ForgeSDKBuilder::register_tool()`.
