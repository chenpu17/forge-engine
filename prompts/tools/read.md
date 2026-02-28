# Read Tool

Reads a file from the local filesystem. You can access any file directly by using this tool.

## Usage

- The `file_path` parameter must be an absolute path, not a relative path
- By default, it reads up to 2000 lines starting from the beginning of the file
- You can optionally specify a line offset and limit (especially handy for long files)
- Any lines longer than 2000 characters will be truncated
- Results are returned with line numbers starting at 1

## Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| file_path | string | Yes | The absolute path to the file to read |
| offset | integer | No | Line number to start reading from (1-indexed) |
| limit | integer | No | Maximum number of lines to read |

## Best Practices

- Always use absolute paths
- For large files, use offset and limit to read specific sections
- You can read multiple files in parallel using the batch tool
- If a file doesn't exist, an error will be returned
- Empty files will return a warning message

## Examples

Read entire file:
```json
{"file_path": "/path/to/file.rs"}
```

Read lines 100-200:
```json
{"file_path": "/path/to/file.rs", "offset": 100, "limit": 100}
```
