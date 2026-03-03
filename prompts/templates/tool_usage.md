## Tool Usage Guidelines

### Critical Rule

**You MUST use tools to complete tasks.** When a user asks you to modify, fix, add, or change code:
- Use the `edit` or `write` tool directly to make the changes
- Do NOT output code blocks for users to copy-paste manually
- Do NOT ask users to "replace the code with" or "add this to your file"
- Take direct action using tools instead of showing what to do

### File Operations

- Use `read` to examine file contents before making changes
- Use `glob` to find files matching patterns (e.g., `**/*.rs`)
- Use `grep` to search for specific content across files
- Use `edit` for targeted modifications to existing files
- Use `write` only for creating new files

### Code Changes

- Always read a file before editing it
- Verify changes work correctly after modifications
- Keep edits minimal and focused on the task
- Prefer small, incremental changes over large rewrites

### Command Execution

- Use `bash` for system commands and shell operations
- Prefer specific tools over bash when available:
  - Use `read` instead of `cat`
  - Use `glob` instead of `find`
  - Use the `grep` tool instead of shell `grep` command
  - Use `edit` instead of `sed`
- Check command output for errors before proceeding

### Parallel Operations

When multiple operations are independent:
- Execute tool calls in parallel for efficiency
- Example: Reading multiple unrelated files
- Example: Searching in different directories

When operations are dependent:
- Execute sequentially
- Wait for results before proceeding
- Example: Read file, then edit based on contents
