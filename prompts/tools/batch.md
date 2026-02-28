# Batch Tool

Execute multiple tools in parallel within a single request.

## Usage

- Maximum 10 tool calls per batch
- Tools are executed in parallel, so they must be independent
- Reduces round-trips and improves efficiency
- Some tools are not allowed: batch, task, ask_user

## Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| tool_calls | array | Yes | Array of tool calls to execute |

Each tool call object:
| Field | Type | Required | Description |
|-------|------|----------|-------------|
| tool | string | Yes | Name of the tool to execute |
| parameters | object | Yes | Parameters for the tool |

## Best Practices

- Use for independent operations that don't depend on each other
- Good for reading multiple files at once
- Good for running multiple grep searches
- Do NOT use for operations where one depends on another's result

## Disallowed Tools

- `batch` - No nested batches
- `task` - Sub-agents should run independently
- `ask_user` - Interactive tools need sequential execution

## Examples

Read multiple files:
```json
{
  "tool_calls": [
    {"tool": "read", "parameters": {"file_path": "/path/to/a.rs"}},
    {"tool": "read", "parameters": {"file_path": "/path/to/b.rs"}},
    {"tool": "read", "parameters": {"file_path": "/path/to/c.rs"}}
  ]
}
```

Multiple searches:
```json
{
  "tool_calls": [
    {"tool": "grep", "parameters": {"pattern": "struct.*Error", "file_type": "rs"}},
    {"tool": "grep", "parameters": {"pattern": "impl.*Error", "file_type": "rs"}}
  ]
}
```
