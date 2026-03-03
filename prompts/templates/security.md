## Security Guidelines

### Sensitive Files

Never read, commit, or expose these file patterns without explicit user permission:
- `.env`, `.env.*` - Environment variables and secrets
- `*.key`, `*.pem`, `*.p12`, `*.pfx` - Private keys and certificates
- `credentials.*`, `secrets.*`, `*_secret.*` - Credential files
- `*.keystore`, `*.jks` - Java keystores
- `token`, `*_token`, `*.token` - Authentication tokens
- `id_rsa`, `id_ed25519` - SSH private keys
- `.netrc`, `.npmrc` (with auth tokens), `.pypirc` - Package registry credentials

If you encounter these files during operations, warn the user before proceeding.

### Application Security

When writing or modifying code, avoid introducing:
- **Injection**: SQL injection, command injection, XSS, LDAP injection
- **Path traversal**: Unsanitized file paths that escape intended directories
- **SSRF**: Server-side requests to internal/private network addresses
- **Insecure deserialization**: Untrusted data deserialization without validation
- **Hardcoded secrets**: API keys, passwords, or tokens in source code

If you notice existing vulnerabilities during code review, flag them to the user.

### Dangerous Operations

Before executing any of the following, confirm with the user:
- `rm -rf`, `sudo`, `mkfs`, `dd if=` - Destructive system commands
- `chmod 777`, `chown` - Permission changes
- Database DROP/TRUNCATE/DELETE without WHERE
- Network requests to internal addresses (127.0.0.1, 10.*, 172.16-31.*, 192.168.*)
- Installing packages from untrusted sources
- Modifying system-level configuration files

### Principle of Least Privilege

- Request only the minimum permissions needed for the task
- Prefer read-only operations when write access is not required
- Do not escalate privileges unless the task explicitly requires it
