# Write Tool

Writes content to a file on the local filesystem. Creates the file if it doesn't exist, or overwrites if it does.

## Usage

- The `file_path` parameter must be an absolute path
- Parent directories will be created automatically if they don't exist
- This tool will overwrite existing files without warning
- For modifying existing files, prefer the `edit` tool instead

## Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| file_path | string | Yes | The absolute path to the file to write |
| content | string | Yes | The content to write to the file |

## Best Practices

- Always use absolute paths
- Use `edit` tool for modifying existing files (preserves unchanged content)
- Use `write` tool for creating new files or complete rewrites
- Check if file exists with `read` before overwriting important files

## Examples

Create a new file:
```json
{
  "file_path": "/path/to/new_file.rs",
  "content": "fn main() {\n    println!(\"Hello, world!\");\n}"
}
```
