//! Tiered context compression for agent message history.
//!
//! Provides a 4-tier compression strategy that progressively reduces context
//! while preserving as much useful information as possible:
//!
//! | Tier | Strategy | Information Retained |
//! |------|----------|---------------------|
//! | 1 | Truncate large tool results (>2KB → 500 chars) | High |
//! | 2 | Remove old tool_use/tool_result pairs, keep text | Medium-High |
//! | 3 | LLM summarize oldest 50% (with timeout) | Medium |
//! | 4 | Aggressive trim to last 10 messages | Low |

use forge_llm::{ChatMessage, ChatRole, ContentBlock, LlmEvent, LlmProvider, MessageContent};
use forge_session::COMPRESSION_PROMPT;
use futures::StreamExt;

use crate::AgentConfig;

/// Reserved tokens for system prompt overhead.
pub const SYSTEM_RESERVED: usize = 4_000;
/// Reserved tokens for LLM response generation.
pub const RESPONSE_RESERVED: usize = 4_096;

/// Threshold (in characters) above which tool results are truncated in Tier 1.
const TIER1_TOOL_RESULT_THRESHOLD: usize = 2_000;
/// Characters to keep when truncating a large tool result.
const TIER1_KEEP_CHARS: usize = 500;

/// Messages to preserve (from the end) during LLM summarization (Tier 3).
const TIER3_KEEP_RECENT: usize = 5;
/// Timeout for the LLM summarization call.
const TIER3_TIMEOUT_SECS: u64 = 30;

/// Maximum messages after aggressive trim (Tier 4).
const TIER4_MAX_MESSAGES: usize = 10;

/// Maximum length for tool result content when preparing compression requests.
const MAX_TOOL_RESULT_LEN_FOR_SUMMARY: usize = 2000;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Estimate tokens for a single `ChatMessage`.
#[must_use]
pub fn estimate_message_tokens(message: &ChatMessage) -> usize {
    let mut total = 0usize;
    match &message.content {
        MessageContent::Text(s) => {
            total += forge_infra::token::estimate_tokens(s);
        }
        MessageContent::Blocks(blocks) => {
            for block in blocks {
                match block {
                    ContentBlock::Text { text } => {
                        total += forge_infra::token::estimate_tokens(text);
                    }
                    ContentBlock::ToolUse { input, .. } => {
                        total += forge_infra::token::estimate_tokens(&input.to_string());
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        total += forge_infra::token::estimate_tokens(content);
                    }
                }
            }
        }
    }
    total + 5 // role/formatting overhead
}

/// Calculate available tokens for messages given a model's context limit.
#[must_use]
pub const fn available_tokens(max_context: usize) -> usize {
    max_context.saturating_sub(SYSTEM_RESERVED).saturating_sub(RESPONSE_RESERVED)
}

/// Trim messages from the oldest end to fit within the context window.
/// Always keeps at least the most recent message.
pub fn trim_to_fit(messages: &[ChatMessage], max_context: usize) -> Vec<ChatMessage> {
    let estimates: Vec<usize> = messages.iter().map(estimate_message_tokens).collect();
    trim_to_fit_with_estimates(messages, &estimates, max_context)
}

/// Trim messages from the oldest end using precomputed per-message token estimates.
///
/// `estimates` must have the same length as `messages` and correspond by index.
pub fn trim_to_fit_with_estimates(
    messages: &[ChatMessage],
    estimates: &[usize],
    max_context: usize,
) -> Vec<ChatMessage> {
    debug_assert_eq!(messages.len(), estimates.len());
    if messages.len() != estimates.len() {
        let recomputed: Vec<usize> = messages.iter().map(estimate_message_tokens).collect();
        return trim_to_fit_with_estimates(messages, &recomputed, max_context);
    }

    let available = available_tokens(max_context);
    let mut total_tokens = 0;
    let mut result = Vec::new();

    for (message, msg_tokens) in messages.iter().rev().zip(estimates.iter().rev()) {
        if total_tokens + *msg_tokens > available && !result.is_empty() {
            tracing::info!(
                "Context trimming: keeping {} of {} messages",
                result.len(),
                messages.len()
            );
            break;
        }
        total_tokens += *msg_tokens;
        result.push(message.clone());
    }

    result.reverse();
    result
}

/// Create a summary message wrapping an LLM-generated summary.
#[must_use]
pub fn create_summary_message(summary: &str) -> ChatMessage {
    ChatMessage {
        role: ChatRole::User,
        content: MessageContent::Text(format!(
            "[Previous Conversation Summary]\n\n{summary}\n\n[End of Summary - Continue from here]"
        )),
    }
}

// ---------------------------------------------------------------------------
// Tiered compression
// ---------------------------------------------------------------------------

/// Apply tiered compression to bring messages within the token budget.
///
/// Tries each tier in order, stopping as soon as the messages fit.
/// Returns the compressed messages and the tier that was applied (0 = no
/// compression needed, 1-4 = tier applied).
pub async fn tiered_compress(
    messages: &[ChatMessage],
    max_context: usize,
    provider: &std::sync::Arc<dyn LlmProvider>,
    config: &AgentConfig,
) -> (Vec<ChatMessage>, u8) {
    let available = available_tokens(max_context);

    // Check if compression is needed at all
    let current: usize = messages.iter().map(estimate_message_tokens).sum();
    if current <= available {
        return (messages.to_vec(), 0);
    }

    // Tier 1: truncate large tool results
    let tier1 = tier1_truncate_tool_results(messages);
    let tier1_tokens: usize = tier1.iter().map(estimate_message_tokens).sum();
    if tier1_tokens <= available {
        tracing::info!(
            "Tier 1 compression: truncated large tool results ({current} -> {tier1_tokens} tokens)"
        );
        return (tier1, 1);
    }

    // Tier 2: remove old tool_use/tool_result pairs, keep text
    let tier2 = tier2_strip_old_tool_pairs(&tier1);
    let tier2_tokens: usize = tier2.iter().map(estimate_message_tokens).sum();
    if tier2_tokens <= available {
        tracing::info!(
            "Tier 2 compression: stripped old tool pairs ({tier1_tokens} -> {tier2_tokens} tokens)"
        );
        return (tier2, 2);
    }

    // Tier 3: LLM summarize oldest 50%
    if let Ok(tier3) = tier3_llm_summarize(&tier2, provider, config).await {
        let tier3_tokens: usize = tier3.iter().map(estimate_message_tokens).sum();
        if tier3_tokens <= available {
            tracing::info!(
                "Tier 3 compression: LLM summarized ({tier2_tokens} -> {tier3_tokens} tokens)"
            );
            return (tier3, 3);
        }
    }

    // Tier 4: aggressive trim (uses tier2 output intentionally — tier3 either
    // failed or didn't shrink enough, so we fall back to the best local result)
    let tier4 = tier4_aggressive_trim(&tier2);
    tracing::warn!("Tier 4 compression: aggressive trim to {} messages", tier4.len());
    (tier4, 4)
}

// ---------------------------------------------------------------------------
// Tier implementations
// ---------------------------------------------------------------------------

/// Tier 1: Truncate tool results larger than `TIER1_TOOL_RESULT_THRESHOLD`.
fn tier1_truncate_tool_results(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    messages
        .iter()
        .map(|msg| match &msg.content {
            MessageContent::Blocks(blocks) => {
                let new_blocks: Vec<ContentBlock> = blocks
                    .iter()
                    .map(|block| match block {
                        ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                            if content.chars().count() > TIER1_TOOL_RESULT_THRESHOLD {
                                let truncated = truncate_chars(content, TIER1_KEEP_CHARS);
                                ContentBlock::ToolResult {
                                    tool_use_id: tool_use_id.clone(),
                                    content: truncated,
                                    is_error: *is_error,
                                }
                            } else {
                                block.clone()
                            }
                        }
                        _ => block.clone(),
                    })
                    .collect();
                ChatMessage { role: msg.role, content: MessageContent::Blocks(new_blocks) }
            }
            MessageContent::Text(_) => msg.clone(),
        })
        .collect()
}

/// Tier 2: Remove `tool_use`/`tool_result` blocks from older messages, keeping
/// only text blocks. Preserves the most recent 30% of messages intact.
fn tier2_strip_old_tool_pairs(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    if messages.is_empty() {
        return Vec::new();
    }

    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let keep_count = (messages.len() as f64 * 0.3).ceil() as usize;
    let strip_end = messages.len().saturating_sub(keep_count);

    let mut result = Vec::with_capacity(messages.len());

    for (i, msg) in messages.iter().enumerate() {
        if i < strip_end {
            match &msg.content {
                MessageContent::Blocks(blocks) => {
                    let text_blocks: Vec<ContentBlock> = blocks
                        .iter()
                        .filter(|b| matches!(b, ContentBlock::Text { .. }))
                        .cloned()
                        .collect();
                    if text_blocks.is_empty() {
                        result.push(ChatMessage {
                            role: msg.role,
                            content: MessageContent::Text(
                                "[tool interaction removed for context compression]".to_string(),
                            ),
                        });
                    } else {
                        result.push(ChatMessage {
                            role: msg.role,
                            content: MessageContent::Blocks(text_blocks),
                        });
                    }
                }
                MessageContent::Text(_) => result.push(msg.clone()),
            }
        } else {
            result.push(msg.clone());
        }
    }

    result
}

/// Tier 3: LLM-based summarization of the oldest 50% of messages.
async fn tier3_llm_summarize(
    messages: &[ChatMessage],
    provider: &std::sync::Arc<dyn LlmProvider>,
    config: &AgentConfig,
) -> std::result::Result<Vec<ChatMessage>, String> {
    if messages.len() <= TIER3_KEEP_RECENT {
        return Ok(messages.to_vec());
    }

    let split_point = messages.len().saturating_sub(TIER3_KEEP_RECENT);
    let to_summarize = &messages[..split_point];
    let to_keep = &messages[split_point..];

    let compression_messages = prepare_compression_request(to_summarize);

    let llm_config = forge_llm::LlmConfig { model: config.model.clone(), ..Default::default() };

    let timeout = std::time::Duration::from_secs(TIER3_TIMEOUT_SECS);
    let stream_result = tokio::time::timeout(
        timeout,
        provider.chat_stream(&compression_messages, vec![], &llm_config),
    )
    .await;

    let mut stream = match stream_result {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(format!("LLM error: {e}")),
        Err(_) => return Err("Compression timeout".to_string()),
    };

    let mut summary = String::new();
    while let Some(event) = stream.next().await {
        if let Ok(LlmEvent::TextDelta(text)) = event {
            summary.push_str(&text);
        }
    }

    if summary.is_empty() {
        return Err("Empty summary".to_string());
    }

    let mut compressed = Vec::with_capacity(to_keep.len() + 1);
    compressed.push(create_summary_message(&summary));
    compressed.extend(to_keep.iter().cloned());
    Ok(compressed)
}

/// Tier 4: Keep only the most recent `TIER4_MAX_MESSAGES` messages.
fn tier4_aggressive_trim(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    if messages.len() <= TIER4_MAX_MESSAGES {
        return messages.to_vec();
    }
    let start = messages.len().saturating_sub(TIER4_MAX_MESSAGES);
    messages[start..].to_vec()
}

// ---------------------------------------------------------------------------
// Helpers (also used by agent.rs for overflow recovery)
// ---------------------------------------------------------------------------

/// Aggressive trim — public wrapper for Tier 4 (used by overflow recovery).
#[must_use]
pub fn aggressive_trim(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    tier4_aggressive_trim(messages)
}

/// Prepare messages for an LLM compression/summarization request.
#[must_use]
pub fn prepare_compression_request(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    let conversation_text = messages
        .iter()
        .map(|m| {
            let role = match m.role {
                ChatRole::User => "User",
                ChatRole::Assistant => "Assistant",
            };
            let content = match &m.content {
                MessageContent::Text(t) => t.clone(),
                MessageContent::Blocks(blocks) => blocks
                    .iter()
                    .map(format_content_block_for_summary)
                    .collect::<Vec<_>>()
                    .join("\n"),
            };
            format!("{role}: {content}")
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    vec![ChatMessage {
        role: ChatRole::User,
        content: MessageContent::Text(format!(
            "{COMPRESSION_PROMPT}\n\nSummarize this conversation:\n\n{conversation_text}"
        )),
    }]
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Truncate a string to `max_chars` characters, appending a suffix.
fn truncate_chars(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{truncated}...[truncated, original {char_count} chars]")
    }
}

/// Format a content block for the summarization prompt.
fn format_content_block_for_summary(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Text { text } => text.clone(),
        ContentBlock::ToolUse { id, name, input } => {
            let input_str = input.to_string();
            let truncated = truncate_chars(&input_str, MAX_TOOL_RESULT_LEN_FOR_SUMMARY);
            format!("[Tool Call: {name} (id: {id})] {truncated}")
        }
        ContentBlock::ToolResult { tool_use_id, content, is_error } => {
            let truncated = truncate_chars(content, MAX_TOOL_RESULT_LEN_FOR_SUMMARY);
            let status = if *is_error { "Error" } else { "Success" };
            format!("[Tool Result (id: {tool_use_id}, {status})] {truncated}")
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn text_msg(role: ChatRole, text: &str) -> ChatMessage {
        ChatMessage { role, content: MessageContent::Text(text.to_string()) }
    }

    fn tool_result_msg(content: &str) -> ChatMessage {
        ChatMessage {
            role: ChatRole::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "t1".to_string(),
                content: content.to_string(),
                is_error: false,
            }]),
        }
    }

    fn tool_use_msg(name: &str) -> ChatMessage {
        ChatMessage {
            role: ChatRole::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: name.to_string(),
                input: serde_json::json!({"key": "value"}),
            }]),
        }
    }

    fn mixed_msg() -> ChatMessage {
        ChatMessage {
            role: ChatRole::Assistant,
            content: MessageContent::Blocks(vec![
                ContentBlock::Text { text: "I'll read the file.".to_string() },
                ContentBlock::ToolUse {
                    id: "t2".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({"path": "src/main.rs"}),
                },
            ]),
        }
    }

    // --- Tier 1 tests ---

    #[test]
    fn tier1_truncates_large_tool_results() {
        let big_content = "x".repeat(3000);
        let messages = vec![tool_result_msg(&big_content)];
        let result = tier1_truncate_tool_results(&messages);

        if let MessageContent::Blocks(blocks) = &result[0].content {
            if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
                assert!(content.len() < big_content.len());
                assert!(content.contains("truncated"));
                return;
            }
        }
        panic!("expected truncated tool result");
    }

    #[test]
    fn tier1_leaves_small_tool_results_unchanged() {
        let small_content = "ok";
        let messages = vec![tool_result_msg(small_content)];
        let result = tier1_truncate_tool_results(&messages);

        if let MessageContent::Blocks(blocks) = &result[0].content {
            if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
                assert_eq!(content, small_content);
                return;
            }
        }
        panic!("expected unchanged tool result");
    }

    #[test]
    fn tier1_leaves_text_messages_unchanged() {
        let messages = vec![text_msg(ChatRole::User, "hello world")];
        let result = tier1_truncate_tool_results(&messages);
        assert_eq!(result.len(), 1);
        if let MessageContent::Text(t) = &result[0].content {
            assert_eq!(t, "hello world");
        } else {
            panic!("expected text message");
        }
    }

    // --- Tier 2 tests ---

    #[test]
    fn tier2_strips_tool_blocks_from_older_messages() {
        // 10 messages: first 7 are "old", last 3 are "recent" (30%)
        let mut messages = Vec::new();
        for i in 0..7 {
            messages.push(tool_use_msg(&format!("tool_{i}")));
        }
        for i in 0..3 {
            messages.push(text_msg(ChatRole::User, &format!("recent {i}")));
        }

        let result = tier2_strip_old_tool_pairs(&messages);
        assert_eq!(result.len(), 10);

        // Old messages should have tool blocks stripped
        for msg in &result[..7] {
            match &msg.content {
                MessageContent::Text(t) => {
                    assert!(t.contains("removed for context compression"));
                }
                MessageContent::Blocks(blocks) => {
                    assert!(blocks.iter().all(|b| matches!(b, ContentBlock::Text { .. })));
                }
            }
        }

        // Recent messages should be untouched
        for (i, msg) in result[7..].iter().enumerate() {
            if let MessageContent::Text(t) = &msg.content {
                assert_eq!(t, &format!("recent {i}"));
            }
        }
    }

    #[test]
    fn tier2_preserves_text_in_mixed_messages() {
        let mut messages = vec![mixed_msg()];
        for _ in 0..5 {
            messages.push(text_msg(ChatRole::User, "recent"));
        }

        let result = tier2_strip_old_tool_pairs(&messages);
        if let MessageContent::Blocks(blocks) = &result[0].content {
            assert_eq!(blocks.len(), 1);
            assert!(
                matches!(&blocks[0], ContentBlock::Text { text } if text.contains("read the file"))
            );
        } else {
            panic!("expected blocks content");
        }
    }

    // --- Tier 4 tests ---

    #[test]
    fn tier4_trims_to_max_messages() {
        let messages: Vec<ChatMessage> =
            (0..20).map(|i| text_msg(ChatRole::User, &format!("msg {i}"))).collect();
        let result = tier4_aggressive_trim(&messages);
        assert_eq!(result.len(), TIER4_MAX_MESSAGES);
        if let MessageContent::Text(t) = &result[0].content {
            assert_eq!(t, "msg 10");
        }
    }

    #[test]
    fn tier4_noop_when_under_limit() {
        let messages: Vec<ChatMessage> =
            (0..5).map(|i| text_msg(ChatRole::User, &format!("msg {i}"))).collect();
        let result = tier4_aggressive_trim(&messages);
        assert_eq!(result.len(), 5);
    }

    // --- Helper tests ---

    #[test]
    fn truncate_chars_short_string() {
        assert_eq!(truncate_chars("hello", 10), "hello");
    }

    #[test]
    fn truncate_chars_long_string() {
        let long = "a".repeat(100);
        let result = truncate_chars(&long, 10);
        assert!(result.starts_with("aaaaaaaaaa"));
        assert!(result.contains("truncated"));
        assert!(result.contains("100"));
    }

    #[test]
    fn estimate_message_tokens_text() {
        let msg = text_msg(ChatRole::User, "hello world");
        let tokens = estimate_message_tokens(&msg);
        assert!(tokens > 0);
    }

    #[test]
    fn available_tokens_subtracts_reserves() {
        let avail = available_tokens(200_000);
        assert_eq!(avail, 200_000 - SYSTEM_RESERVED - RESPONSE_RESERVED);
    }

    #[test]
    fn trim_to_fit_keeps_recent() {
        // Create messages that exceed a tiny context window
        let messages: Vec<ChatMessage> =
            (0..10).map(|i| text_msg(ChatRole::User, &"x".repeat(500 * (i + 1)))).collect();
        let result = trim_to_fit(&messages, 20_000);
        // Should keep fewer messages than original
        assert!(result.len() <= messages.len());
        assert!(!result.is_empty());
    }

    #[test]
    fn create_summary_message_format() {
        let msg = create_summary_message("test summary");
        if let MessageContent::Text(t) = &msg.content {
            assert!(t.contains("test summary"));
            assert!(t.contains("Previous Conversation Summary"));
        } else {
            panic!("expected text content");
        }
    }

    #[test]
    fn format_content_block_for_summary_text() {
        let block = ContentBlock::Text { text: "hello".to_string() };
        assert_eq!(format_content_block_for_summary(&block), "hello");
    }

    #[test]
    fn format_content_block_for_summary_tool_use() {
        let block = ContentBlock::ToolUse {
            id: "t1".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "ls"}),
        };
        let result = format_content_block_for_summary(&block);
        assert!(result.contains("bash"));
        assert!(result.contains("t1"));
    }

    #[test]
    fn format_content_block_for_summary_tool_result() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "t1".to_string(),
            content: "file.txt".to_string(),
            is_error: false,
        };
        let result = format_content_block_for_summary(&block);
        assert!(result.contains("file.txt"));
        assert!(result.contains("Success"));
    }
}
