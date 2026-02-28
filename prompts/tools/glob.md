# Glob Tool

Fast file pattern matching tool for finding files by name patterns.

## Usage

- Supports standard glob patterns like `**/*.rs`, `src/**/*.ts`
- Returns matching file paths sorted by modification time
- Respects `.gitignore` rules by default
- Use this when you need to find files by name patterns

## Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| pattern | string | Yes | The glob pattern to match files against |
| path | string | No | Directory to search in (default: current working directory) |

## Pattern Syntax

| Pattern | Matches |
|---------|---------|
| `*` | Any sequence of characters (except `/`) |
| `**` | Any sequence of characters (including `/`) |
| `?` | Any single character |
| `[abc]` | Any character in the set |
| `[!abc]` | Any character not in the set |

## Best Practices

- Use `**` for recursive searches: `**/*.rs`
- Be specific to reduce results: `src/**/*.rs` instead of `**/*.rs`
- Combine with grep for content search after finding files

## Examples

Find all Rust files:
```json
{"pattern": "**/*.rs"}
```

Find test files in src:
```json
{"pattern": "src/**/*_test.rs"}
```

Find config files:
```json
{"pattern": "**/config.{json,toml,yaml}"}
```
