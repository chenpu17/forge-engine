//! Secure secret storage using system keychain
//!
//! # Key Naming Convention
//!
//! - MCP API Key:        `forge:mcp:<server_name>:api_key`
//! - MCP Proxy Password: `forge:mcp:<server_name>:proxy_password`
//! - Global Proxy:       `forge:proxy:global`
//!
//! # Platform Support
//!
//! - macOS: Keychain (via security command)
//! - Windows: Credential Manager (via keyring crate)
//! - Linux: Secret Service (via keyring crate)

use thiserror::Error;

/// Service name for keyring entries
const SERVICE_NAME: &str = "forge";

/// Secret storage errors
#[derive(Debug, Error)]
pub enum SecretError {
    /// Failed to access keychain
    #[error("Keychain access failed: {0}")]
    KeychainError(String),

    /// Secret not found
    #[error("Secret not found: {0}")]
    NotFound(String),

    /// Platform not supported
    #[error("Platform not supported for secure storage")]
    PlatformNotSupported,
}

/// Result type for secret operations
pub type SecretResult<T> = Result<T, SecretError>;

/// Secret store trait for abstracting storage backends
pub trait SecretStore: Send + Sync {
    /// Get a secret by key
    fn get(&self, key: &str) -> SecretResult<Option<String>>;
    /// Set a secret
    fn set(&self, key: &str, value: &str) -> SecretResult<()>;
    /// Delete a secret
    fn delete(&self, key: &str) -> SecretResult<()>;
}

/// System keychain-based secret store
pub struct KeychainStore;

impl KeychainStore {
    /// Create a new keychain store
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Build a keyring entry for the given key (non-macOS)
    #[cfg(not(target_os = "macos"))]
    fn entry(&self, key: &str) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(SERVICE_NAME, key)
            .map_err(|e| SecretError::KeychainError(e.to_string()))
    }
}

impl Default for KeychainStore {
    fn default() -> Self {
        Self::new()
    }
}

// macOS implementation using security command
#[cfg(target_os = "macos")]
impl SecretStore for KeychainStore {
    fn get(&self, key: &str) -> SecretResult<Option<String>> {
        use std::process::Command;

        let output = Command::new("security")
            .args(["find-generic-password", "-s", SERVICE_NAME, "-a", key, "-w"])
            .output()
            .map_err(|e| SecretError::KeychainError(format!("Failed to run security: {e}")))?;

        if output.status.success() {
            let password = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(Some(password))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("could not be found") || stderr.contains("SecKeychainSearchCopyNext")
            {
                Ok(None)
            } else {
                Err(SecretError::KeychainError(stderr.to_string()))
            }
        }
    }

    fn set(&self, key: &str, value: &str) -> SecretResult<()> {
        use std::process::Command;

        // Use -X flag to pass password as hex string (avoids exposing raw password)
        let hex_password: String = value.as_bytes().iter().map(|b| format!("{b:02x}")).collect();

        let output = Command::new("security")
            .args([
                "add-generic-password",
                "-s",
                SERVICE_NAME,
                "-a",
                key,
                "-X",
                &hex_password,
                "-U", // Update if exists, create if not
            ])
            .output()
            .map_err(|e| SecretError::KeychainError(format!("Failed to run security: {e}")))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(SecretError::KeychainError(stderr.to_string()))
        }
    }

    fn delete(&self, key: &str) -> SecretResult<()> {
        use std::process::Command;

        let output = Command::new("security")
            .args(["delete-generic-password", "-s", SERVICE_NAME, "-a", key])
            .output()
            .map_err(|e| SecretError::KeychainError(format!("Failed to run security: {e}")))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("could not be found") || stderr.contains("SecKeychainSearchCopyNext")
            {
                Ok(())
            } else {
                Err(SecretError::KeychainError(stderr.to_string()))
            }
        }
    }
}

// Non-macOS implementation using keyring crate
#[cfg(not(target_os = "macos"))]
impl SecretStore for KeychainStore {
    fn get(&self, key: &str) -> SecretResult<Option<String>> {
        let entry = self.entry(key)?;
        match entry.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(SecretError::KeychainError(e.to_string())),
        }
    }

    fn set(&self, key: &str, value: &str) -> SecretResult<()> {
        let entry = self.entry(key)?;
        entry.set_password(value).map_err(|e| SecretError::KeychainError(e.to_string()))
    }

    fn delete(&self, key: &str) -> SecretResult<()> {
        let entry = self.entry(key)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(SecretError::KeychainError(e.to_string())),
        }
    }
}

// Key builders

/// Build the keychain key for an MCP service API key
#[must_use]
pub fn mcp_api_key(server_name: &str) -> String {
    format!("forge:mcp:{server_name}:api_key")
}

/// Build the keychain key for an MCP service proxy password
#[must_use]
pub fn mcp_proxy_password(server_name: &str) -> String {
    format!("forge:mcp:{server_name}:proxy_password")
}

/// The keychain key for global proxy password
pub const GLOBAL_PROXY_KEY: &str = "forge:proxy:global";

/// Get the default secret store
#[must_use]
pub fn default_store() -> KeychainStore {
    KeychainStore::new()
}

/// Get an MCP service API key from keychain
///
/// # Errors
/// Returns error if keychain access fails
pub fn get_mcp_api_key(server_name: &str) -> SecretResult<Option<String>> {
    default_store().get(&mcp_api_key(server_name))
}

/// Set an MCP service API key in keychain
///
/// # Errors
/// Returns error if keychain access fails
pub fn set_mcp_api_key(server_name: &str, api_key: &str) -> SecretResult<()> {
    default_store().set(&mcp_api_key(server_name), api_key)
}

/// Delete an MCP service API key from keychain
///
/// # Errors
/// Returns error if keychain access fails
pub fn delete_mcp_api_key(server_name: &str) -> SecretResult<()> {
    default_store().delete(&mcp_api_key(server_name))
}

/// Get an MCP service proxy password from keychain
///
/// # Errors
/// Returns error if keychain access fails
pub fn get_mcp_proxy_password(server_name: &str) -> SecretResult<Option<String>> {
    default_store().get(&mcp_proxy_password(server_name))
}

/// Set an MCP service proxy password in keychain
///
/// # Errors
/// Returns error if keychain access fails
pub fn set_mcp_proxy_password(server_name: &str, password: &str) -> SecretResult<()> {
    default_store().set(&mcp_proxy_password(server_name), password)
}

/// Delete an MCP service proxy password from keychain
///
/// # Errors
/// Returns error if keychain access fails
pub fn delete_mcp_proxy_password(server_name: &str) -> SecretResult<()> {
    default_store().delete(&mcp_proxy_password(server_name))
}

/// Get global proxy password from keychain
///
/// # Errors
/// Returns error if keychain access fails
pub fn get_global_proxy_password() -> SecretResult<Option<String>> {
    default_store().get(GLOBAL_PROXY_KEY)
}

/// Set global proxy password in keychain
///
/// # Errors
/// Returns error if keychain access fails
pub fn set_global_proxy_password(password: &str) -> SecretResult<()> {
    default_store().set(GLOBAL_PROXY_KEY, password)
}

/// Delete global proxy password from keychain
///
/// # Errors
/// Returns error if keychain access fails
pub fn delete_global_proxy_password() -> SecretResult<()> {
    default_store().delete(GLOBAL_PROXY_KEY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_builders() {
        assert_eq!(mcp_api_key("bocha"), "forge:mcp:bocha:api_key");
        assert_eq!(mcp_proxy_password("external"), "forge:mcp:external:proxy_password");
        assert_eq!(GLOBAL_PROXY_KEY, "forge:proxy:global");
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[test]
    #[ignore] // Run manually: cargo test -p forge-infra secret::integration_tests --ignored
    fn test_keychain_write_read() {
        let server_name = "test-server";
        let api_key = "test-api-key-12345";

        let write_result = set_mcp_api_key(server_name, api_key);
        assert!(write_result.is_ok(), "Failed to write: {:?}", write_result);

        let read_result = get_mcp_api_key(server_name);
        assert!(read_result.is_ok(), "Failed to read: {:?}", read_result);

        let value = read_result.expect("read should succeed");
        assert_eq!(value, Some(api_key.to_string()), "Value mismatch");

        let _ = delete_mcp_api_key(server_name);
    }
}
