# Task Tool

Launch sub-agents to handle complex, multi-step tasks autonomously.

## Usage

- Spawns specialized agents for different task types
- Sub-agents have access to a subset of tools
- Results are returned when the sub-agent completes
- Use for complex searches or multi-step operations

## Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| subagent_type | string | Yes | Type of sub-agent to spawn |
| prompt | string | Yes | The task description for the sub-agent |

## Sub-Agent Types

| Type | Description | Use Case |
|------|-------------|----------|
| explore | Fast project exploration | Finding files, understanding structure |
| plan | Design implementation plans | Architecture decisions, planning |
| research | In-depth research | Complex questions, documentation |
| general | General purpose | Multi-step tasks, complex searches |
| writer | Content creation and writing | Documents, reports, proposals |
| analyst | Data analysis and visualization | Statistical analysis, data reports |

## Best Practices

- Use `explore` for quick file/code searches
- Use `plan` for designing implementation approaches
- Use `writer` for content creation and document writing
- Use `analyst` for data analysis and report generation
- Provide detailed prompts with clear objectives
- Sub-agents work independently, no back-and-forth

## Examples

Explore project:
```json
{
  "subagent_type": "explore",
  "prompt": "Find all files that handle user authentication"
}
```

Plan implementation:
```json
{
  "subagent_type": "plan",
  "prompt": "Design an approach to add caching to the API layer"
}
```
