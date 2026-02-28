# WebSearch Tool

Search the web for information.

## Usage

- Performs web searches and returns results
- Supports multiple search providers
- Use for finding documentation, solutions, references
- Automatic retry on network failures

## Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| query | string | Yes | The search query |
| num_results | integer | No | Number of results to return (default: 5) |

## Best Practices

- Use specific, targeted queries
- Include relevant keywords (language, framework, version)
- Combine with web_fetch to read full content
- Good for finding documentation and examples

## Examples

Search for documentation:
```json
{"query": "rust tokio async runtime tutorial"}
```

Find error solutions:
```json
{"query": "rust borrow checker error E0382 solution"}
```

Search with more results:
```json
{"query": "react hooks best practices 2024", "num_results": 10}
```
