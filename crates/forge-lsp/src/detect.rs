//! Language server detection and configuration
//!
//! Detects available language servers based on project files and PATH.

use std::path::Path;

/// Known language server configuration
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Language identifier
    pub language: &'static str,
    /// Command to run
    pub command: &'static str,
    /// Command arguments
    pub args: &'static [&'static str],
    /// File extensions this server handles
    pub extensions: &'static [&'static str],
    /// Marker files that indicate this language is used in the project
    pub marker_files: &'static [&'static str],
}

/// Known language server configurations
const KNOWN_SERVERS: &[ServerConfig] = &[
    ServerConfig {
        language: "rust",
        command: "rust-analyzer",
        args: &[],
        extensions: &["rs"],
        marker_files: &["Cargo.toml", "Cargo.lock"],
    },
    ServerConfig {
        language: "typescript",
        command: "typescript-language-server",
        args: &["--stdio"],
        extensions: &["ts", "tsx", "js", "jsx"],
        marker_files: &["tsconfig.json", "package.json", "jsconfig.json"],
    },
    ServerConfig {
        language: "python",
        command: "pyright-langserver",
        args: &["--stdio"],
        extensions: &["py", "pyi"],
        marker_files: &["pyproject.toml", "setup.py", "setup.cfg", "requirements.txt", "Pipfile"],
    },
    ServerConfig {
        language: "go",
        command: "gopls",
        args: &["serve"],
        extensions: &["go"],
        marker_files: &["go.mod", "go.sum"],
    },
];

/// Detect which language servers are relevant for a project directory
///
/// Returns configs for servers whose marker files exist in the project root.
#[must_use]
pub fn detect_servers(project_dir: &Path) -> Vec<&'static ServerConfig> {
    KNOWN_SERVERS
        .iter()
        .filter(|config| config.marker_files.iter().any(|marker| project_dir.join(marker).exists()))
        .collect()
}

/// Find the server config for a given file extension
#[must_use]
pub fn server_for_extension(ext: &str) -> Option<&'static ServerConfig> {
    KNOWN_SERVERS.iter().find(|config| config.extensions.contains(&ext))
}

/// Find the server config for a given language identifier
#[must_use]
pub fn server_for_language(language: &str) -> Option<&'static ServerConfig> {
    KNOWN_SERVERS.iter().find(|config| config.language == language)
}

/// Check if a command is available in PATH
#[must_use]
pub fn is_command_available(command: &str) -> bool {
    if let Ok(path_var) = std::env::var("PATH") {
        let separator = if cfg!(windows) { ';' } else { ':' };
        for dir in path_var.split(separator) {
            let candidate = Path::new(dir).join(command);
            if candidate.is_file() {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_server_for_extension() {
        assert_eq!(server_for_extension("rs").map(|s| s.language), Some("rust"));
        assert_eq!(server_for_extension("ts").map(|s| s.language), Some("typescript"));
        assert_eq!(server_for_extension("py").map(|s| s.language), Some("python"));
        assert_eq!(server_for_extension("go").map(|s| s.language), Some("go"));
        assert!(server_for_extension("xyz").is_none());
    }

    #[test]
    fn test_server_for_language() {
        let config = server_for_language("rust").expect("rust config");
        assert_eq!(config.command, "rust-analyzer");
        assert!(config.extensions.contains(&"rs"));

        let config = server_for_language("typescript").expect("ts config");
        assert_eq!(config.command, "typescript-language-server");

        assert!(server_for_language("unknown").is_none());
    }

    #[test]
    fn test_detect_servers_with_cargo() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").expect("write");

        let servers = detect_servers(dir.path());
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].language, "rust");
    }

    #[test]
    fn test_detect_servers_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let servers = detect_servers(dir.path());
        assert!(servers.is_empty());
    }

    #[test]
    fn test_detect_servers_multiple() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Cargo.toml"), "").expect("write");
        std::fs::write(dir.path().join("package.json"), "{}").expect("write");

        let servers = detect_servers(dir.path());
        assert_eq!(servers.len(), 2);
        let languages: Vec<_> = servers.iter().map(|s| s.language).collect();
        assert!(languages.contains(&"rust"));
        assert!(languages.contains(&"typescript"));
    }
}
