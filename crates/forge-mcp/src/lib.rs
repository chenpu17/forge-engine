//! MCP (Model Context Protocol) client for the Forge AI agent engine.
//!
//! This crate provides a complete MCP client implementation:
//! - [`client::McpClient`] — High-level client for MCP server interaction
//! - [`transport`] — Transport layers (Stdio, SSE, Streamable HTTP)
//! - [`types`] — JSON-RPC and MCP protocol types
//! - [`wrapper::McpToolWrapper`] — Adapts MCP tools to `forge_domain::Tool`
//! - [`wrapper::McpManager`] — Manages multiple MCP server connections
//! - [`security`] — Input validation and security checks
//! - [`health`] — Circuit breaker for server health management
//! - [`auth`] — OAuth 2.1 authentication (PKCE)

pub mod auth;
pub mod client;
pub mod health;
pub mod security;
pub mod transport;
pub mod types;
pub mod wrapper;

// Re-export commonly used types at crate root.
pub use client::{McpClient, McpClientConfig, McpClientError};
pub use health::CircuitBreaker;
pub use transport::{AuthHeader, ProxyConfig, TransportError};
pub use types::McpTool;
pub use wrapper::{
    ApiKeyAuth, McpConfig, McpManager, McpServerConfig, McpToolWrapper, McpTransportType,
};
