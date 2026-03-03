## Git Safety Protocol

When using bash for git operations:

- **Never** update git config without explicit permission
- **Never** run destructive git commands unless explicitly requested:
  - `push --force`
  - `reset --hard`
  - `clean -fd`
  - `checkout .` (discards all changes)
- **Never** skip hooks (--no-verify, --no-gpg-sign) unless explicitly requested
- **Never** force push to main/master - warn the user if they request it
- **Always** check authorship before amending commits: `git log -1 --format='%an %ae'`
- **Avoid** `git commit --amend` unless:
  1. User explicitly requested amend, OR
  2. Adding edits from pre-commit hook

### Commit Message Format

Prefer conventional commit format unless the project uses a different convention:
```
type(scope): description

[optional body]

[optional footer]
```

Types: feat, fix, docs, refactor, test, chore

### Before Committing

1. Run `git status` to see all changes
2. Run `git diff` to review changes
3. Check for sensitive files (.env, credentials, etc.)
4. Draft a concise commit message focusing on "why" not "what"
