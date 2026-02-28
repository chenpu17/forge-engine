# AskUser Tool

Ask the user questions to gather information or clarify requirements.

## Usage

- Present structured questions with predefined options
- Gather user preferences or decisions
- Clarify ambiguous instructions
- Users can always provide custom input

## Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| question | string | Yes | The question to ask |
| options | array | No | Predefined answer options |

Each option:
| Field | Type | Required | Description |
|-------|------|----------|-------------|
| label | string | Yes | Short display text |
| description | string | No | Longer explanation |

## Best Practices

- Ask clear, specific questions
- Provide 2-4 meaningful options
- Include descriptions for complex choices
- Users can always type custom responses

## When to Use

- Multiple valid implementation approaches
- Unclear requirements
- Need user preference on trade-offs
- Confirming understanding before major changes

## Examples

Ask about approach:
```json
{
  "question": "Which authentication method should we use?",
  "options": [
    {"label": "JWT", "description": "Stateless, good for APIs"},
    {"label": "Session", "description": "Server-side, traditional web apps"},
    {"label": "OAuth", "description": "Third-party authentication"}
  ]
}
```

Simple confirmation:
```json
{
  "question": "Should I proceed with refactoring the database layer?",
  "options": [
    {"label": "Yes", "description": "Proceed with the refactoring"},
    {"label": "No", "description": "Cancel and discuss alternatives"}
  ]
}
```
