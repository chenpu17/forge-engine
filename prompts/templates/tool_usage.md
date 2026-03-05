## Tool Usage Guidelines

### Critical Rule - THIS IS A HARD REQUIREMENT

**You MUST use tools to complete tasks.** When a user asks you to modify, fix, add, or change code:
- Use the `edit` or `write` tool directly to make the changes
- Do NOT output code blocks for users to copy-paste manually
- Do NOT ask users to "replace the code with" or "add this to your file"
- Take direct action using tools instead of showing what to do

**Do NOT use bash commands when dedicated tools exist:**
- Use `read` instead of `cat/head/tail/sed`
- Use `edit` instead of `sed/awk`
- Use `write` instead of `echo/cat` redirection
- Use `glob` instead of `find/ls`
- Use `grep` tool instead of shell `grep/rg`

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

**Maximize use of parallel tool calls** where possible to increase efficiency.

When operations are **independent** (no dependencies):
- Execute ALL independent tool calls in parallel in a single response
- Example: Reading multiple unrelated files
- Example: Searching in different directories

When operations are **dependent** (one needs results from another):
- Execute sequentially
- Wait for results before proceeding
- Example: Read file, then edit based on contents

### Sub-Agent Usage

When delegating tasks to sub-agents:

- **Avoid duplicating work**: If you delegate research to a sub-agent, do not also perform the same searches yourself. Trust the sub-agent's results.
- **Use for isolation**: Sub-agents are valuable for parallelizing independent queries or protecting the main context window from excessive results.
- **Don't overuse**: For simple, directed searches (e.g., finding a specific file), use Glob or Grep directly. Reserve sub-agents for broader exploration or complex multi-step tasks.
- **Provide clear context**: When delegating, give the sub-agent sufficient context and a clear goal.
