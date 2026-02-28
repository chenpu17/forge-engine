# Bash Tool

Executes shell commands in a persistent bash session.

## Usage

- Commands run in the current working directory
- Environment variables from the session are available
- Commands have a configurable timeout (default: 120 seconds)
- Output is captured and returned (stdout and stderr combined)

## Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| command | string | Yes | The bash command to execute |
| timeout | integer | No | Timeout in seconds (default: 120, max: 600) |

## Best Practices

- Use absolute paths when possible
- Quote paths with spaces: `cd "/path/with spaces"`
- Chain dependent commands with `&&`: `mkdir -p dir && cd dir`
- For file operations, prefer dedicated tools (read, write, edit)
- Avoid interactive commands that require user input

## Security Notes

- Dangerous commands require user confirmation
- Commands that modify system files are flagged
- Network commands may be restricted

## Examples

Run a build:
```json
{"command": "cargo build --release"}
```

Check git status:
```json
{"command": "git status"}
```

Run tests with timeout:
```json
{"command": "cargo test", "timeout": 300}
```
