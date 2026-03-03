//! Context management for sessions
//!
//! Handles context window management, token counting, and context compression.

use crate::{ContentBlock, Message, MessageContent, MessageRole};

/// Shared compression prompt for LLM-based summarization
pub const COMPRESSION_PROMPT: &str = r"Summarize this conversation to preserve important context while reducing tokens.

Focus on:
1. User's primary goal
2. Key files and code mentioned
3. Completed work
4. Current state and next steps

Output a concise summary (under 2000 tokens) that captures essential technical details.";

/// Context window configuration
#[derive(Debug, Clone)]
pub struct ContextConfig {
    /// Maximum tokens in context
    pub max_tokens: usize,
    /// Reserved tokens for system prompt
    pub system_reserved: usize,
    /// Reserved tokens for response
    pub response_reserved: usize,
    /// Threshold for triggering compression (percentage of `max_tokens`)
    pub compression_threshold: f32,
    /// Target after compression (percentage of `max_tokens`)
    pub compression_target: f32,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_tokens: 200_000,
            system_reserved: 4_000,
            response_reserved: 4_096,
            compression_threshold: 0.8, // Compress when at 80% capacity
            compression_target: 0.5,    // Target 50% capacity after compression
        }
    }
}

/// Compression strategy for context management
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompressionStrategy {
    /// Drop oldest messages (simple, lossy)
    #[default]
    DropOldest,
    /// Summarize older messages into a summary message
    Summarize,
    /// Keep important messages, drop less important ones
    SelectiveRetention,
}

/// Result of a compression operation
#[derive(Debug, Clone)]
pub struct CompressionResult {
    /// Number of messages before compression
    pub messages_before: usize,
    /// Number of messages after compression
    pub messages_after: usize,
    /// Estimated tokens before compression
    pub tokens_before: usize,
    /// Estimated tokens after compression
    pub tokens_after: usize,
    /// Summary of dropped/compressed content (if any)
    pub summary: Option<String>,
}

/// Context manager for managing conversation context
#[derive(Debug)]
pub struct ContextManager {
    config: ContextConfig,
    strategy: CompressionStrategy,
}

impl ContextManager {
    /// Create a new context manager
    #[must_use]
    pub fn new(config: ContextConfig) -> Self {
        Self { config, strategy: CompressionStrategy::default() }
    }

    /// Set the compression strategy
    #[must_use]
    pub const fn with_strategy(mut self, strategy: CompressionStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Get available tokens for messages
    #[must_use]
    pub const fn available_tokens(&self) -> usize {
        self.config
            .max_tokens
            .saturating_sub(self.config.system_reserved)
            .saturating_sub(self.config.response_reserved)
    }

    /// Get the compression threshold in tokens
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    pub fn compression_threshold_tokens(&self) -> usize {
        (self.available_tokens() as f32 * self.config.compression_threshold) as usize
    }

    /// Get the compression target in tokens
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    pub fn compression_target_tokens(&self) -> usize {
        (self.available_tokens() as f32 * self.config.compression_target) as usize
    }

    /// Estimate tokens for a message (simple estimation)
    ///
    /// Delegates to the unified `forge_infra::estimate_tokens`.
    #[must_use]
    pub fn estimate_tokens(text: &str) -> usize {
        forge_infra::estimate_tokens(text)
    }

    /// Estimate total tokens for a list of messages
    #[must_use]
    pub fn estimate_total_tokens(messages: &[Message]) -> usize {
        messages.iter().map(Self::estimate_message_tokens).sum()
    }

    /// Check if compression is needed
    #[must_use]
    pub fn needs_compression(&self, messages: &[Message]) -> bool {
        let total = Self::estimate_total_tokens(messages);
        total >= self.compression_threshold_tokens()
    }

    /// Compress context using the configured strategy
    #[must_use]
    pub fn compress(&self, messages: &[Message]) -> (Vec<Message>, CompressionResult) {
        let tokens_before = Self::estimate_total_tokens(messages);
        let messages_before = messages.len();

        let (compressed, summary) = match self.strategy {
            CompressionStrategy::DropOldest => (self.compress_drop_oldest(messages), None),
            CompressionStrategy::Summarize => self.compress_with_summary(messages),
            CompressionStrategy::SelectiveRetention => (self.compress_selective(messages), None),
        };

        let tokens_after = Self::estimate_total_tokens(&compressed);
        let messages_after = compressed.len();

        let result = CompressionResult {
            messages_before,
            messages_after,
            tokens_before,
            tokens_after,
            summary,
        };

        (compressed, result)
    }

    /// Trim messages to fit within context window
    ///
    /// Keeps the most recent messages that fit within the available tokens.
    /// Always keeps at least the most recent message.
    #[must_use]
    pub fn trim_to_fit(&self, messages: &[Message]) -> Vec<Message> {
        let available = self.available_tokens();
        let mut total_tokens = 0;
        let mut result = Vec::new();

        // Process messages from newest to oldest
        for message in messages.iter().rev() {
            let message_tokens = Self::estimate_message_tokens(message);

            if total_tokens + message_tokens > available && !result.is_empty() {
                // Would exceed limit and we have at least one message
                break;
            }

            total_tokens += message_tokens;
            result.push(message.clone());
        }

        // Reverse to restore chronological order
        result.reverse();
        result
    }

    /// Compress by dropping oldest messages
    fn compress_drop_oldest(&self, messages: &[Message]) -> Vec<Message> {
        let target = self.compression_target_tokens();
        let mut total_tokens = 0;
        let mut result = Vec::new();

        // Keep messages from newest to oldest until we reach target
        for message in messages.iter().rev() {
            let message_tokens = Self::estimate_message_tokens(message);

            if total_tokens + message_tokens > target && !result.is_empty() {
                break;
            }

            total_tokens += message_tokens;
            result.push(message.clone());
        }

        result.reverse();
        result
    }

    /// Compress with summarization
    fn compress_with_summary(&self, messages: &[Message]) -> (Vec<Message>, Option<String>) {
        let target = self.compression_target_tokens();

        // First, find the split point
        let mut total_tokens = 0;
        let mut keep_from = messages.len();

        for (i, message) in messages.iter().rev().enumerate() {
            let message_tokens = Self::estimate_message_tokens(message);
            if total_tokens + message_tokens > target {
                // Keep at least the most recent message.
                keep_from = if i == 0 {
                    messages.len().saturating_sub(1)
                } else {
                    messages.len().saturating_sub(i)
                };
                break;
            }
            total_tokens += message_tokens;
        }

        // If we can keep everything, no compression needed
        if keep_from == messages.len() {
            return (messages.to_vec(), None);
        }

        // Create a summary of dropped messages
        let dropped_messages = &messages[..keep_from];
        let summary = Self::create_summary(dropped_messages);

        // Build result with summary + kept messages
        let mut result = Vec::with_capacity(messages.len() - keep_from + 1);

        // Add summary as a system message
        result.push(Message {
            role: MessageRole::System,
            content: MessageContent::Text(format!("[Context Summary]\n{summary}")),
            timestamp: dropped_messages.first().map_or_else(chrono::Utc::now, |m| m.timestamp),
        });

        // Add kept messages
        result.extend(messages[keep_from..].iter().cloned());

        (result, Some(summary))
    }

    /// Compress by selecting important messages
    fn compress_selective(&self, messages: &[Message]) -> Vec<Message> {
        let target = self.compression_target_tokens();

        // Score each message by importance
        let scored: Vec<(usize, &Message, usize)> = messages
            .iter()
            .enumerate()
            .map(|(i, msg)| (i, msg, Self::score_importance(msg, i, messages.len())))
            .collect();

        // Sort by importance (descending) while keeping recent messages high priority
        let mut sorted = scored.clone();
        sorted.sort_by(|a, b| b.2.cmp(&a.2));

        // Select messages until we hit target
        let mut selected_indices: Vec<usize> = Vec::new();
        let mut total_tokens = 0;

        for (idx, msg, _score) in sorted {
            let msg_tokens = Self::estimate_message_tokens(msg);
            if total_tokens + msg_tokens > target && !selected_indices.is_empty() {
                break;
            }
            total_tokens += msg_tokens;
            selected_indices.push(idx);
        }

        // Sort by original order
        selected_indices.sort_unstable();

        // Build result
        selected_indices.iter().map(|&i| messages[i].clone()).collect()
    }

    /// Score message importance
    fn score_importance(message: &Message, index: usize, total: usize) -> usize {
        let mut score = 0;

        // Recency bonus (newer messages are more important)
        let recency_score = (index * 100) / total.max(1);
        score += recency_score;

        // Role-based scoring
        score += match message.role {
            MessageRole::System => 50,    // System messages are important
            MessageRole::User => 30,      // User messages are important for context
            MessageRole::Assistant => 20, // Assistant responses less so
        };

        // Content-based scoring
        let text = message.text();

        // Longer messages might be more substantial
        if text.len() > 500 {
            score += 10;
        }

        // Messages with code are often important
        if text.contains("```") || text.contains("fn ") || text.contains("def ") {
            score += 20;
        }

        // Messages with errors or important keywords
        if text.contains("error")
            || text.contains("Error")
            || text.contains("fix")
            || text.contains("important")
        {
            score += 15;
        }

        score
    }

    /// Create a summary of messages
    fn create_summary(messages: &[Message]) -> String {
        let mut summary_parts = Vec::new();

        // Count messages by role
        let user_count = messages.iter().filter(|m| m.role == MessageRole::User).count();
        let assistant_count = messages.iter().filter(|m| m.role == MessageRole::Assistant).count();

        summary_parts.push(format!(
            "Previous conversation: {user_count} user messages, {assistant_count} assistant responses."
        ));

        // Extract file paths mentioned (common patterns)
        let mut files_mentioned: Vec<String> = Vec::new();
        for msg in messages {
            let text = msg.text();
            // Match common file path patterns
            for word in text.split_whitespace() {
                if (word.contains('/') || word.contains('\\'))
                    && (std::path::Path::new(word).extension().is_some_and(|ext| {
                        ext.eq_ignore_ascii_case("rs")
                            || ext.eq_ignore_ascii_case("ts")
                            || ext.eq_ignore_ascii_case("tsx")
                            || ext.eq_ignore_ascii_case("js")
                            || ext.eq_ignore_ascii_case("json")
                            || ext.eq_ignore_ascii_case("md")
                            || ext.eq_ignore_ascii_case("toml")
                            || ext.eq_ignore_ascii_case("py")
                    }))
                {
                    let clean = word.trim_matches(|c: char| {
                        !c.is_alphanumeric()
                            && c != '/'
                            && c != '\\'
                            && c != '.'
                            && c != '_'
                            && c != '-'
                    });
                    if !clean.is_empty() && !files_mentioned.contains(&clean.to_string()) {
                        files_mentioned.push(clean.to_string());
                    }
                }
            }
        }
        if !files_mentioned.is_empty() {
            let files_str = files_mentioned.iter().take(10).cloned().collect::<Vec<_>>().join(", ");
            summary_parts.push(format!("Files referenced: {files_str}"));
        }

        // Extract key user requests (last 5 user messages)
        let user_requests: Vec<String> = messages
            .iter()
            .filter(|m| m.role == MessageRole::User)
            .rev()
            .take(5)
            .map(|m| {
                let text = m.text();
                let truncated = if text.len() > 150 {
                    format!("{}...", &text.chars().take(150).collect::<String>())
                } else {
                    text
                };
                truncated.replace('\n', " ")
            })
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        if !user_requests.is_empty() {
            summary_parts.push("Recent user requests:".to_string());
            for (i, req) in user_requests.iter().enumerate() {
                summary_parts.push(format!("  {}. {}", i + 1, req));
            }
        }

        summary_parts.join("\n")
    }

    /// Estimate tokens for a message
    fn estimate_message_tokens(message: &Message) -> usize {
        let mut base_tokens = 0usize;

        match &message.content {
            MessageContent::Text(text) => {
                base_tokens += Self::estimate_tokens(text);
            }
            MessageContent::Blocks(blocks) => {
                for block in blocks {
                    match block {
                        ContentBlock::Text { text } => {
                            base_tokens += Self::estimate_tokens(text);
                        }
                        ContentBlock::ToolUse { input, .. } => {
                            base_tokens += Self::estimate_tokens(&input.to_string());
                        }
                        ContentBlock::ToolResult { content, .. } => {
                            base_tokens += Self::estimate_tokens(content);
                        }
                    }
                }
            }
        }

        // Add overhead for role and formatting
        let overhead = match message.role {
            MessageRole::System => 10,
            MessageRole::User | MessageRole::Assistant => 5,
        };

        base_tokens + overhead
    }
}

impl Default for ContextManager {
    fn default() -> Self {
        Self::new(ContextConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_message(role: MessageRole, text: &str) -> Message {
        Message { role, content: MessageContent::Text(text.to_string()), timestamp: Utc::now() }
    }

    #[test]
    fn test_estimate_tokens() {
        // English text
        let english = "Hello, this is a test message.";
        let tokens = ContextManager::estimate_tokens(english);
        assert!(tokens > 0);
        assert!(tokens < english.len()); // Should be fewer tokens than chars

        // Chinese text
        let chinese = "你好，这是一条测试消息。";
        let chinese_tokens = ContextManager::estimate_tokens(chinese);
        assert!(chinese_tokens > 0);
    }

    #[test]
    fn test_available_tokens() {
        let config = ContextConfig {
            max_tokens: 100_000,
            system_reserved: 5_000,
            response_reserved: 4_000,
            ..Default::default()
        };
        let manager = ContextManager::new(config);
        assert_eq!(manager.available_tokens(), 91_000);
    }

    #[test]
    fn test_trim_to_fit() {
        let config = ContextConfig {
            max_tokens: 100,
            system_reserved: 10,
            response_reserved: 10,
            ..Default::default()
        };
        let manager = ContextManager::new(config);

        let messages: Vec<Message> =
            (0..10).map(|i| make_message(MessageRole::User, &format!("Message {}", i))).collect();

        let trimmed = manager.trim_to_fit(&messages);
        // Should have kept some messages but not all
        assert!(!trimmed.is_empty());
        assert!(trimmed.len() <= messages.len());
    }

    #[test]
    fn test_needs_compression() {
        let config = ContextConfig {
            max_tokens: 100,
            system_reserved: 10,
            response_reserved: 10,
            compression_threshold: 0.5,
            compression_target: 0.3,
        };
        let manager = ContextManager::new(config);

        // Small amount of messages - no compression needed
        let few_messages = vec![make_message(MessageRole::User, "Hello")];
        assert!(!manager.needs_compression(&few_messages));

        // Many messages - compression needed
        let many_messages: Vec<Message> = (0..50)
            .map(|i| {
                make_message(MessageRole::User, &format!("This is a longer message number {}", i))
            })
            .collect();
        assert!(manager.needs_compression(&many_messages));
    }

    #[test]
    fn test_compress_drop_oldest() {
        let config = ContextConfig {
            max_tokens: 200,
            system_reserved: 10,
            response_reserved: 10,
            compression_threshold: 0.8,
            compression_target: 0.5,
        };
        let manager = ContextManager::new(config).with_strategy(CompressionStrategy::DropOldest);

        let messages: Vec<Message> =
            (0..20).map(|i| make_message(MessageRole::User, &format!("Message {}", i))).collect();

        let (compressed, result) = manager.compress(&messages);

        assert!(compressed.len() < messages.len());
        assert!(result.tokens_after < result.tokens_before);
        // Last message should be preserved
        assert_eq!(compressed.last().unwrap().text(), "Message 19");
    }

    #[test]
    fn test_compress_with_summary() {
        let config = ContextConfig {
            max_tokens: 500,
            system_reserved: 10,
            response_reserved: 10,
            compression_threshold: 0.8,
            compression_target: 0.3,
        };
        let manager = ContextManager::new(config).with_strategy(CompressionStrategy::Summarize);

        let messages: Vec<Message> = (0..20)
            .map(|i| {
                make_message(
                    if i % 2 == 0 { MessageRole::User } else { MessageRole::Assistant },
                    &format!("This is message number {} with some content", i),
                )
            })
            .collect();

        let (compressed, result) = manager.compress(&messages);

        // Should have a summary message
        assert!(result.summary.is_some());
        // First message should be the summary
        assert!(compressed[0].text().contains("[Context Summary]"));
    }

    #[test]
    fn test_compress_with_summary_keeps_latest_message() {
        let config = ContextConfig {
            max_tokens: 200,
            system_reserved: 10,
            response_reserved: 10,
            compression_threshold: 0.6,
            compression_target: 0.2,
        };
        let manager = ContextManager::new(config).with_strategy(CompressionStrategy::Summarize);

        let messages: Vec<Message> = (0..15)
            .map(|i| make_message(MessageRole::User, &format!("Message {} with extra content", i)))
            .collect();

        let (compressed, result) = manager.compress(&messages);

        assert!(result.summary.is_some());
        assert_eq!(compressed.last().unwrap().text(), messages.last().unwrap().text());
    }

    #[test]
    fn test_estimate_message_tokens_with_blocks() {
        use serde_json::json;

        let text_message = Message {
            role: MessageRole::User,
            content: MessageContent::Text("Hello world".to_string()),
            timestamp: Utc::now(),
        };
        let blocks_message = Message {
            role: MessageRole::User,
            content: MessageContent::Blocks(vec![
                ContentBlock::Text { text: "Hello world".to_string() },
                ContentBlock::ToolUse {
                    id: "1".to_string(),
                    name: "tool".to_string(),
                    input: json!({"a": 1}),
                },
                ContentBlock::ToolResult {
                    tool_use_id: "1".to_string(),
                    content: "ok".to_string(),
                    is_error: false,
                },
            ]),
            timestamp: Utc::now(),
        };

        let text_tokens = ContextManager::estimate_message_tokens(&text_message);
        let block_tokens = ContextManager::estimate_message_tokens(&blocks_message);

        assert!(block_tokens > text_tokens);
    }

    #[test]
    fn test_score_importance() {
        let messages = vec![
            make_message(MessageRole::System, "System prompt"),
            make_message(MessageRole::User, "User question"),
            make_message(MessageRole::Assistant, "Response with ```code```"),
        ];

        // System message should have high importance
        let system_score = ContextManager::score_importance(&messages[0], 0, 3);
        let _user_score = ContextManager::score_importance(&messages[1], 1, 3);
        let code_score = ContextManager::score_importance(&messages[2], 2, 3);

        // Code content should boost importance
        assert!(code_score > 0);
        // System messages have role bonus
        assert!(system_score >= 50);
    }

    #[test]
    fn test_compression_result() {
        let config = ContextConfig {
            max_tokens: 200,
            system_reserved: 10,
            response_reserved: 10,
            compression_threshold: 0.8,
            compression_target: 0.5,
        };
        let manager = ContextManager::new(config);

        let messages: Vec<Message> =
            (0..20).map(|i| make_message(MessageRole::User, &format!("Message {}", i))).collect();

        let (_, result) = manager.compress(&messages);

        assert_eq!(result.messages_before, 20);
        assert!(result.messages_after < result.messages_before);
        assert!(result.tokens_after <= result.tokens_before);
    }
}
