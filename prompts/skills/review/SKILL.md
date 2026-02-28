---
name: "review"
display-name: "Code Review"
description: "Review current code changes and provide actionable improvement suggestions. Use when user wants code review."
allowed-tools: [Bash, Read, Glob, Grep]
version: "1.0.0"
argument-hint: "[files]"
disable-model-invocation: false
user-invocable: true
tags: ["code-quality", "workflow"]
---

Please do a code review of the current working directory changes:

1. View change contents (cover both staged and unstaged)
2. Review from the perspectives of correctness, maintainability, style consistency, performance, and security
3. Output in the format: "Issue -> Impact -> Suggested fix (provide specific file/location/fix when possible)"

Output format:

### Overall Assessment
[Brief summary]

### Issue List
- [ ] Issue 1: Description (file:line) - Impact - Fix suggestion
- [ ] Issue 2: Description (file:line) - Impact - Fix suggestion

### Suggestions (optional)
1. Suggestion 1
2. Suggestion 2
