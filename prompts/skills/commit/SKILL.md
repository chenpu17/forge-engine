---
name: "commit"
display-name: "Git Commit"
description: "Analyze changes and create git commits with AI-generated messages. Use when user wants to commit changes."
allowed-tools: [Bash, Read, Glob, Grep]
version: "1.0.0"
argument-hint: "[message]"
disable-model-invocation: false
user-invocable: true
tags: ["git", "workflow"]
---

Please help me complete a high-quality git commit:

1. Check working directory status (untracked/unstaged/staged)
2. View change contents (cover both staged and unstaged)
3. View the last 5 commits to infer the project's commit message style and conventions
4. **If the user provided a commit message argument**: use it directly as the commit message (skip candidate generation)
   **If no message was provided**: generate 1-3 candidate commit messages based on changes (follow the project's existing commit style if consistent; if no clear pattern, fall back to conventional commits format `type(scope): description`), and recommend one
5. Perform `git add` and `git commit` with the final message

Constraints:
- Do not include any Forge/Claude/LLM related terms in the commit message
- Do not auto `git push` unless explicitly requested
- If the repository has pre-commit/pre-push hooks that fail, fix and retry
