# Symbols Tool

Search for symbol definitions (functions, classes, structs, etc.) in source code.

## Usage

- Finds code definitions using language-aware patterns
- More precise than grep for finding where things are defined
- Supports filtering by symbol type and name pattern
- Respects .gitignore rules

## Supported Languages

- Rust (.rs)
- TypeScript/JavaScript (.ts, .tsx, .js, .jsx)
- Python (.py)
- Go (.go)
- Java (.java)
- C/C++ (.c, .cpp, .cc, .cxx, .h, .hpp)

## Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| name | string | No | Name pattern to search for (regex supported) |
| type | string | No | Symbol type: function, class, struct, enum, interface, trait, const, type, module, all |
| path | string | No | Directory or file to search in (default: cwd) |
| file_type | string | No | File extension filter (e.g., "rs", "ts") |

## Best Practices

- Use `type` filter to narrow results: `type: "function"`
- Use `name` pattern for specific searches: `name: "handle.*"`
- Combine with `file_type` for language-specific searches
- Results are limited to 100 symbols

## Examples

Find all functions:
```json
{"type": "function"}
```

Find structs matching a pattern:
```json
{"name": "Error", "type": "struct", "file_type": "rs"}
```

Find all definitions in a directory:
```json
{"path": "src/core", "type": "all"}
```

Find trait implementations:
```json
{"name": "impl.*Tool", "type": "trait", "file_type": "rs"}
```
