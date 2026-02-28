# Git Tool

Execute Git operations in the current repository.

## Usage

- Provides safe access to common Git operations
- Some operations require user confirmation
- Supports status, diff, log, add, commit, branch operations

## Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| operation | string | Yes | The Git operation to perform |
| args | array | No | Additional arguments for the operation |

## Supported Operations

| Operation | Description | Confirmation |
|-----------|-------------|--------------|
| status | Show working tree status | None |
| diff | Show changes | None |
| log | Show commit history | None |
| add | Stage files | None |
| commit | Create a commit | Once |
| branch | List/create branches | None |
| checkout | Switch branches | Once |
| push | Push to remote | Always |
| pull | Pull from remote | Once |

## Best Practices

- Always check `status` before committing
- Review `diff` before staging changes
- Use descriptive commit messages
- Never force push to main/master

## Examples

Check status:
```json
{"operation": "status"}
```

View recent commits:
```json
{"operation": "log", "args": ["-5", "--oneline"]}
```

Stage and commit:
```json
{"operation": "add", "args": ["."]}
```
```json
{"operation": "commit", "args": ["-m", "feat: add new feature"]}
```
