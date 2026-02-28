# TodoWrite Tool

Manage a structured task list for tracking progress on complex tasks.

## Usage

- Create and update task lists during coding sessions
- Track progress on multi-step operations
- Show the user what you're working on
- Mark tasks as pending, in_progress, or completed

## Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| todos | array | Yes | Array of todo items |

Each todo item:
| Field | Type | Required | Description |
|-------|------|----------|-------------|
| content | string | Yes | Task description (imperative form) |
| status | string | Yes | One of: pending, in_progress, completed |

## Best Practices

- Use for tasks with 3+ steps
- Keep only ONE task as `in_progress` at a time
- Mark tasks complete immediately when done
- Update the list as you discover new tasks
- Remove tasks that are no longer relevant

## When to Use

- Complex multi-step implementations
- Bug fixes requiring multiple changes
- Refactoring across multiple files
- User provides a list of tasks

## When NOT to Use

- Single, simple tasks
- Quick questions or explanations
- Tasks completable in one step

## Examples

Create initial task list:
```json
{
  "todos": [
    {"content": "Read existing code", "status": "in_progress"},
    {"content": "Implement new feature", "status": "pending"},
    {"content": "Add tests", "status": "pending"},
    {"content": "Update documentation", "status": "pending"}
  ]
}
```
