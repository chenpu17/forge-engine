# Edit Tool

Performs precise text replacements in files. Use this for modifying existing files.

## Usage

- Replaces exact occurrences of `old_string` with `new_string`
- The `old_string` must match exactly (including whitespace and indentation)
- If `old_string` is not found or matches multiple locations, the edit will fail
- For creating new files, use the `write` tool instead

## Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| file_path | string | Yes | The absolute path to the file to edit |
| old_string | string | Yes | The exact text to find and replace |
| new_string | string | Yes | The text to replace with |
| replace_all | boolean | No | If true, replace all occurrences (default: false) |

## Best Practices

- Always read the file first to understand its current content
- Include enough context in `old_string` to make it unique
- Preserve exact indentation (tabs/spaces) from the original file
- Use `replace_all: true` for renaming variables across a file
- For multiple edits in the same file, make them sequentially

## Common Mistakes

- Not including enough context, causing ambiguous matches
- Wrong indentation in `old_string` or `new_string`
- Trying to edit a file that hasn't been read yet

## Examples

Replace a function:
```json
{
  "file_path": "/path/to/file.rs",
  "old_string": "fn old_name() {\n    // old code\n}",
  "new_string": "fn new_name() {\n    // new code\n}"
}
```

Rename a variable everywhere:
```json
{
  "file_path": "/path/to/file.rs",
  "old_string": "old_var",
  "new_string": "new_var",
  "replace_all": true
}
```
