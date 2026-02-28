# Persona Guide

## Overview

Personas define the agent's role, tool set, and system prompt. They are configured via TOML files in `configs/personas/`.

## Built-in Personas

| Persona | Tools | Use Case |
|---------|-------|----------|
| assistant | All builtin | General-purpose assistant |
| coder | All + forge-tools-coding | Software development |
| analyst | read, glob, grep, bash, write, web_search | Data analysis |
| writer | read, write, edit, web_search, web_fetch | Writing tasks |
| devops | All builtin | Infrastructure & operations |
| researcher | glob, grep, read, web_fetch, web_search | Deep research |
| commander | Orchestration only | Multi-agent coordination |

## Creating a Custom Persona

1. Create `configs/personas/my_persona.toml`:

```toml
[persona]
name = "my_persona"
display_name = "My Custom Persona"
description = "Specialized for X"
system_prompt_file = "prompts/personas/my_persona.md"

[tools]
include = ["read", "write", "edit", "bash", "glob", "grep"]
```

2. Create `prompts/personas/my_persona.md` with the system prompt.

3. Set as default in config: `default_persona = "my_persona"`
