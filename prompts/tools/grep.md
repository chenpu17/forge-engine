# Grep Tool

Powerful content search tool built on ripgrep for searching text patterns in files.

## Usage

- Supports full regex syntax
- Can filter by file type or glob pattern
- Returns matching lines with context
- Use this when you need to find content within files

## Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| pattern | string | Yes | The regex pattern to search for |
| path | string | No | File or directory to search in (default: cwd) |
| file_type | string | No | File type filter (e.g., "rs", "ts", "py") |
| glob | string | No | Glob pattern to filter files |
| context | integer | No | Lines of context around matches (default: 2) |
| case_sensitive | boolean | No | Case sensitive search (default: true) |

## Regex Syntax

- Literal text: `function`
- Word boundary: `\bword\b`
- Any character: `.`
- Character class: `[a-z]`
- Quantifiers: `*`, `+`, `?`, `{n,m}`
- Alternation: `foo|bar`
- Groups: `(pattern)`

## Best Practices

- Use word boundaries for precise matches: `\bfn\b`
- Filter by file type to reduce noise: `file_type: "rs"`
- Use context to understand matches better
- For simple searches, literal strings work fine

## Examples

Find function definitions:
```json
{"pattern": "fn\\s+\\w+", "file_type": "rs"}
```

Find TODO comments:
```json
{"pattern": "TODO|FIXME", "context": 1}
```

Search in specific directory:
```json
{"pattern": "import.*React", "path": "src/components", "file_type": "tsx"}
```
