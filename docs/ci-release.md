# CI/CD & Release

## CI Pipeline

Triggered on PRs and pushes to `main`. Jobs:

- **fmt** — `cargo fmt --check`
- **clippy** — `cargo clippy --workspace`
- **test** — `cargo test --workspace` (Linux, macOS, Windows)
- **audit** — `cargo audit` for known vulnerabilities

## Release Pipeline

Triggered by pushing a `v*` tag (e.g., `v0.1.0`).

### Steps

1. Tag the release: `git tag v0.1.0 && git push --tags`
2. CI builds artifacts for all platforms
3. NAPI `.node` binaries built per platform
4. Python wheels built via maturin per platform
5. GitHub Release created with all artifacts

### Platforms

| Target | OS |
|--------|----|
| aarch64-apple-darwin | macOS ARM |
| x86_64-apple-darwin | macOS Intel |
| x86_64-unknown-linux-gnu | Linux x64 |
| x86_64-pc-windows-msvc | Windows x64 |

### Secrets Required

- `NPM_TOKEN` — for npm publish
- `PYPI_TOKEN` — for PyPI publish
