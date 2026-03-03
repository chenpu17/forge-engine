//! Result reflector
//!
//! Analyzes tool results, classifies errors, and provides
//! recovery suggestions and retry decisions.

use crate::checkpoint::GitCheckpointManager;
use crate::episodic_memory::{EpisodeRecord, EpisodicMemoryStore};
use crate::{AgentError, LoopProtectionConfig, ReflectionConfig};
use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use chrono::Utc;
use forge_domain::{AgentEvent, ToolResult};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Error classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// Tool not found
    NotFound,
    /// Invalid input/parameters
    InvalidInput,
    /// Permission denied
    PermissionDenied,
    /// Resource not found (file, URL, etc.)
    ResourceNotFound,
    /// Network/connection error
    NetworkError,
    /// Timeout
    Timeout,
    /// Rate limit exceeded
    RateLimited,
    /// Context/token overflow
    ContextOverflow,
    /// Generic execution error
    ExecutionError,
    /// Unknown error type
    Unknown,
}

impl ErrorKind {
    /// Check if this error is retryable
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::NetworkError | Self::Timeout | Self::RateLimited)
    }

    /// Get suggested delay before retry (in seconds)
    #[must_use]
    pub const fn retry_delay(&self) -> Option<u64> {
        match self {
            Self::NetworkError => Some(1),
            Self::Timeout => Some(2),
            Self::RateLimited => Some(30),
            _ => None,
        }
    }
}

/// Structured error signature for deduplication.
///
/// Two signatures are considered equal if they share the same `(kind, tool)` pair,
/// regardless of the specific file involved. This groups "same tool, same error type"
/// together for repetition detection.
#[derive(Debug, Clone)]
pub struct ErrorSignature {
    /// Classified error kind
    pub kind: ErrorKind,
    /// Tool that produced the error
    pub tool: String,
    /// Optional file path extracted from the error message (informational, not used for equality)
    pub file: Option<String>,
}

impl PartialEq for ErrorSignature {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.tool == other.tool
    }
}

impl Eq for ErrorSignature {}

impl Hash for ErrorSignature {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.kind.hash(state);
        self.tool.hash(state);
    }
}

/// Extract a file path hint from an error message.
///
/// Looks for common path patterns (absolute paths, relative paths with extensions).
fn extract_file_hint(output: &str) -> Option<String> {
    // Look for absolute or relative paths in the error output
    for word in output.split_whitespace() {
        let trimmed = word.trim_matches(|c: char| c == '\'' || c == '"' || c == ':' || c == ',');
        if (trimmed.starts_with('/') || trimmed.starts_with("./") || trimmed.starts_with("../"))
            && trimmed.contains('.')
            && trimmed.len() > 2
        {
            return Some(trimmed.to_string());
        }
    }
    None
}

/// Recovery action to take after an error
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Retry the same operation after a delay
    Retry {
        /// Delay before retrying
        delay: Duration,
        /// Maximum number of retries remaining
        max_retries: usize,
    },
    /// Skip this operation and continue with others
    Skip,
    /// Report the error to the user and continue
    ReportAndContinue {
        /// Message to show the user
        message: String,
    },
    /// Ask the LLM to try a different approach
    TryAlternative {
        /// Hint for what to try instead
        hint: String,
    },
    /// Attempt context compression and retry
    TryCompression {
        /// Hint about compression
        hint: String,
    },
    /// Roll back the working tree to the checkpoint and retry with a fresh approach
    Rollback {
        /// Reason for the rollback
        reason: String,
    },
    /// Stop the agent due to unrecoverable error
    Stop {
        /// Reason for stopping
        reason: String,
    },
}

/// A classification rule: a set of patterns that map to an `ErrorKind`.
///
/// Rules are evaluated in priority order (first match wins).
/// Each rule requires ALL `required` patterns to match, and NONE of the `exclude` patterns.
struct ClassificationRule {
    kind: ErrorKind,
    /// All of these patterns must be present (AND logic)
    required: &'static [&'static str],
    /// If any of these patterns are present, this rule is skipped
    exclude: &'static [&'static str],
}

/// Compiled classification index for fast multi-pattern matching.
struct ClassificationIndex {
    matcher: AhoCorasick,
    rules: Vec<CompiledRule>,
}

/// A rule compiled to pattern indices in the automaton.
struct CompiledRule {
    kind: ErrorKind,
    required: Vec<usize>,
    exclude: Vec<usize>,
}

/// Structured error classification rules, ordered by specificity (most specific first).
///
/// Rules use multi-word phrases to reduce false positives compared to single-word matching.
/// Exit codes are also checked as a secondary signal.
static CLASSIFICATION_RULES: &[ClassificationRule] = &[
    // --- High specificity: HTTP status codes and structured patterns ---
    ClassificationRule { kind: ErrorKind::RateLimited, required: &["rate limit"], exclude: &[] },
    ClassificationRule { kind: ErrorKind::RateLimited, required: &["status", "429"], exclude: &[] },
    ClassificationRule { kind: ErrorKind::RateLimited, required: &["error", "429"], exclude: &[] },
    // --- Tool/command not found (precise phrases before generic "not found") ---
    ClassificationRule {
        kind: ErrorKind::NotFound,
        required: &["command not found"],
        exclude: &[],
    },
    ClassificationRule { kind: ErrorKind::NotFound, required: &["exit code: 127"], exclude: &[] },
    ClassificationRule { kind: ErrorKind::NotFound, required: &["tool not found"], exclude: &[] },
    ClassificationRule { kind: ErrorKind::NotFound, required: &["unknown tool"], exclude: &[] },
    // --- Not found (localized shell/OS messages) ---
    ClassificationRule { kind: ErrorKind::NotFound, required: &["未找到命令"], exclude: &[] },
    ClassificationRule { kind: ErrorKind::NotFound, required: &["找不到命令"], exclude: &[] },
    ClassificationRule {
        kind: ErrorKind::NotFound,
        required: &["不是内部或外部命令"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::NotFound,
        required: &["コマンド", "見つかりません"],
        exclude: &[],
    },
    // --- Permission errors ---
    ClassificationRule {
        kind: ErrorKind::PermissionDenied,
        required: &["permission denied"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::PermissionDenied,
        required: &["access denied"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::PermissionDenied,
        required: &["unauthorized"],
        exclude: &["not found"],
    },
    ClassificationRule {
        kind: ErrorKind::PermissionDenied,
        required: &["status", "401"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::PermissionDenied,
        required: &["status", "403"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::PermissionDenied,
        required: &["exit code: 126"],
        exclude: &[],
    },
    // --- Permission errors (localized) ---
    ClassificationRule {
        kind: ErrorKind::PermissionDenied,
        required: &["权限被拒绝"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::PermissionDenied, required: &["拒绝访问"], exclude: &[]
    },
    ClassificationRule {
        kind: ErrorKind::PermissionDenied,
        required: &["アクセス", "拒否"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::PermissionDenied,
        required: &["許可", "ありません"],
        exclude: &[],
    },
    // --- Resource not found ---
    ClassificationRule {
        kind: ErrorKind::ResourceNotFound,
        required: &["does not exist"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::ResourceNotFound,
        required: &["no such file"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::ResourceNotFound,
        required: &["not found"],
        exclude: &["command", "tool"],
    },
    ClassificationRule {
        kind: ErrorKind::ResourceNotFound,
        required: &["status", "404"],
        exclude: &[],
    },
    // --- Resource not found (localized) ---
    ClassificationRule {
        kind: ErrorKind::ResourceNotFound,
        required: &["没有那个文件或目录"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::ResourceNotFound,
        required: &["找不到文件"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::ResourceNotFound,
        required: &["ファイル", "見つかりません"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::ResourceNotFound,
        required: &["そのようなファイルやディレクトリはありません"],
        exclude: &[],
    },
    // --- Timeout ---
    ClassificationRule { kind: ErrorKind::Timeout, required: &["timed out"], exclude: &[] },
    ClassificationRule { kind: ErrorKind::Timeout, required: &["timeout"], exclude: &["context"] },
    ClassificationRule { kind: ErrorKind::Timeout, required: &["超时"], exclude: &[] },
    ClassificationRule {
        kind: ErrorKind::Timeout, required: &["タイムアウト"], exclude: &[]
    },
    // --- Rate limiting (additional patterns) ---
    ClassificationRule {
        kind: ErrorKind::RateLimited,
        required: &["too many requests"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::RateLimited, required: &["请求过于频繁"], exclude: &[]
    },
    ClassificationRule {
        kind: ErrorKind::RateLimited,
        required: &["リクエスト", "多すぎ"],
        exclude: &[],
    },
    // --- Network errors (use multi-word phrases to avoid false positives) ---
    ClassificationRule {
        kind: ErrorKind::NetworkError,
        required: &["connection refused"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::NetworkError,
        required: &["connection reset"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::NetworkError,
        required: &["dns resolution"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::NetworkError,
        required: &["network error"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::NetworkError,
        required: &["network unreachable"],
        exclude: &[],
    },
    // --- Network errors (localized) ---
    ClassificationRule {
        kind: ErrorKind::NetworkError, required: &["连接被拒绝"], exclude: &[]
    },
    ClassificationRule {
        kind: ErrorKind::NetworkError, required: &["连接被重置"], exclude: &[]
    },
    ClassificationRule {
        kind: ErrorKind::NetworkError, required: &["无法解析主机"], exclude: &[]
    },
    ClassificationRule {
        kind: ErrorKind::NetworkError, required: &["接続", "拒否"], exclude: &[]
    },
    ClassificationRule { kind: ErrorKind::NetworkError, required: &["名前解決"], exclude: &[] },
    // --- Context overflow (use specific phrases, not bare "context") ---
    ClassificationRule {
        kind: ErrorKind::ContextOverflow,
        required: &["token limit"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::ContextOverflow,
        required: &["context length"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::ContextOverflow,
        required: &["too many tokens"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::ContextOverflow,
        required: &["context overflow"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::ContextOverflow,
        required: &["上下文长度"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::ContextOverflow,
        required: &["上下文", "超出"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::ContextOverflow,
        required: &["コンテキスト", "長すぎ"],
        exclude: &[],
    },
    // --- Invalid input (use specific phrases, not bare "invalid") ---
    ClassificationRule {
        kind: ErrorKind::InvalidInput,
        required: &["missing required"],
        exclude: &[],
    },
    ClassificationRule { kind: ErrorKind::InvalidInput, required: &["bad request"], exclude: &[] },
    ClassificationRule {
        kind: ErrorKind::InvalidInput,
        required: &["invalid parameter"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::InvalidInput,
        required: &["invalid argument"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::InvalidInput,
        required: &["invalid input"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::InvalidInput,
        required: &["invalid option"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::InvalidInput,
        required: &["invalid value"],
        exclude: &[],
    },
    ClassificationRule {
        kind: ErrorKind::InvalidInput, required: &["缺少必需参数"], exclude: &[]
    },
    ClassificationRule { kind: ErrorKind::InvalidInput, required: &["无效参数"], exclude: &[] },
    ClassificationRule {
        kind: ErrorKind::InvalidInput, required: &["無効な引数"], exclude: &[]
    },
    // --- Execution errors (exit code based) ---
    ClassificationRule {
        kind: ErrorKind::ExecutionError,
        required: &["exit code:"],
        exclude: &["exit code: 0"],
    },
];

/// Classify an error message into an error kind using structured rules.
///
/// Rules are evaluated in priority order (first match wins). Each rule uses
/// multi-word phrases to reduce false positives. Falls back to `Unknown` if
/// no rule matches.
fn classify_error(message: &str) -> ErrorKind {
    let msg_lower = message.to_lowercase();
    let index = classification_index();
    let mut matched = vec![false; index.matcher.patterns_len()];

    for m in index.matcher.find_overlapping_iter(&msg_lower) {
        matched[m.pattern().as_usize()] = true;
    }

    for rule in &index.rules {
        let all_required = rule.required.iter().all(|&p| matched[p]);
        let any_excluded = rule.exclude.iter().any(|&p| matched[p]);
        if all_required && !any_excluded {
            return rule.kind;
        }
    }

    ErrorKind::Unknown
}

fn classification_index() -> &'static ClassificationIndex {
    static INDEX: OnceLock<ClassificationIndex> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut patterns: Vec<&'static str> = Vec::new();
        let mut pattern_ids: HashMap<&'static str, usize> = HashMap::new();
        let mut compiled_rules = Vec::with_capacity(CLASSIFICATION_RULES.len());

        let mut intern_pattern = |pat: &'static str| -> usize {
            if let Some(&idx) = pattern_ids.get(pat) {
                idx
            } else {
                let idx = patterns.len();
                patterns.push(pat);
                pattern_ids.insert(pat, idx);
                idx
            }
        };

        for rule in CLASSIFICATION_RULES {
            let required = rule.required.iter().map(|p| intern_pattern(p)).collect();
            let exclude = rule.exclude.iter().map(|p| intern_pattern(p)).collect();
            compiled_rules.push(CompiledRule { kind: rule.kind, required, exclude });
        }

        #[allow(clippy::expect_used)]
        let matcher = AhoCorasickBuilder::new()
            .match_kind(MatchKind::Standard)
            .build(patterns)
            .expect("classification patterns must compile");
        ClassificationIndex { matcher, rules: compiled_rules }
    })
}

/// Analysis result for a tool execution
#[derive(Debug, Clone)]
pub struct ReflectionResult {
    /// Was the execution successful
    pub success: bool,
    /// Error kind if failed
    pub error_kind: Option<ErrorKind>,
    /// Should retry
    pub should_retry: bool,
    /// Suggested delay before retry
    pub retry_delay: Option<u64>,
    /// Suggestion for recovery
    pub suggestion: Option<String>,
    /// Recommended recovery action
    pub recovery_action: RecoveryAction,
}

/// Result reflector analyzes tool results and provides recovery guidance
#[derive(Debug)]
pub struct Reflector {
    /// Configuration
    config: ReflectionConfig,
    /// Count of results by tool
    tool_counts: HashMap<String, usize>,
    /// Count of errors by tool
    error_counts: HashMap<String, usize>,
    /// Error patterns seen (structured signature -> count)
    error_patterns: HashMap<ErrorSignature, usize>,
    /// Consecutive failure count
    consecutive_failures: usize,
    /// Total calls
    total_calls: usize,
    /// Total errors
    total_errors: usize,
    /// Whether a git checkpoint exists (enables Rollback instead of Stop)
    has_checkpoint: bool,
    /// Number of rollbacks performed so far
    rollback_count: usize,
    /// Maximum allowed rollbacks before falling back to Stop
    max_rollbacks: usize,
}

impl Default for Reflector {
    fn default() -> Self {
        Self::new()
    }
}

impl Reflector {
    /// Create a new reflector with default configuration
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(ReflectionConfig::default())
    }

    /// Create a new reflector with custom configuration
    #[must_use]
    pub fn with_config(config: ReflectionConfig) -> Self {
        Self {
            config,
            tool_counts: HashMap::new(),
            error_counts: HashMap::new(),
            error_patterns: HashMap::new(),
            consecutive_failures: 0,
            total_calls: 0,
            total_errors: 0,
            has_checkpoint: false,
            rollback_count: 0,
            max_rollbacks: 2,
        }
    }

    /// Record a tool result with the tool name for per-tool tracking
    pub fn record_result(&mut self, result: &ToolResult, tool_name: &str) {
        self.total_calls += 1;

        let tool_name = tool_name.to_string();
        *self.tool_counts.entry(tool_name.clone()).or_insert(0) += 1;

        if result.is_error {
            self.total_errors += 1;
            self.consecutive_failures += 1;
            *self.error_counts.entry(tool_name.clone()).or_insert(0) += 1;

            // Track error patterns with structured signature
            let kind = classify_error(&result.output);
            let file = extract_file_hint(&result.output);
            let sig = ErrorSignature { kind, tool: tool_name, file };
            *self.error_patterns.entry(sig).or_insert(0) += 1;
        } else {
            self.consecutive_failures = 0;
        }
    }

    /// Check if the output looks like a test failure (vs a system error)
    fn is_test_failure(output: &str) -> bool {
        let lower = output.to_lowercase();
        // Common test failure patterns
        (lower.contains("test") || lower.contains("spec") || lower.contains("assert"))
            && (lower.contains("failed") || lower.contains("failure") || lower.contains("error"))
    }

    /// Analyze a tool result
    #[must_use]
    pub fn analyze(&self, result: &ToolResult, tool_name: &str) -> ReflectionResult {
        if !result.is_error {
            return ReflectionResult {
                success: true,
                error_kind: None,
                should_retry: false,
                retry_delay: None,
                suggestion: None,
                recovery_action: RecoveryAction::Skip, // No action needed for success
            };
        }

        let error_kind = classify_error(&result.output);
        let should_retry = error_kind.is_retryable();
        let retry_delay = error_kind.retry_delay();

        let suggestion = Self::suggest_recovery(error_kind, &result.output);
        let recovery_action = self.determine_recovery_action(error_kind, &result.output, tool_name);

        ReflectionResult {
            success: false,
            error_kind: Some(error_kind),
            should_retry,
            retry_delay,
            suggestion,
            recovery_action,
        }
    }

    /// Determine the appropriate recovery action based on error type
    fn determine_recovery_action(
        &self,
        kind: ErrorKind,
        output: &str,
        tool_name: &str,
    ) -> RecoveryAction {
        // Use different thresholds for test failures vs system errors
        let threshold = if Self::is_test_failure(output) {
            self.config.max_test_failure_count
        } else {
            self.config.max_same_error_count
        };

        // Check if we've seen this error too many times
        if self.is_pattern_repeating(output, tool_name, threshold) {
            let error_type =
                if Self::is_test_failure(output) { "test failure pattern" } else { "error" };
            let reason = format!("Same {error_type} repeated {threshold} times");
            return self.stop_or_rollback(reason);
        }

        // Check consecutive failures - use higher threshold for test failures
        let consecutive_threshold = if Self::is_test_failure(output) {
            self.config.max_consecutive_test_failures
        } else {
            self.config.max_consecutive_failures
        };
        if self.consecutive_failures >= consecutive_threshold {
            let reason = format!("Too many consecutive failures ({})", self.consecutive_failures);
            return self.stop_or_rollback(reason);
        }

        match kind {
            ErrorKind::NetworkError => {
                RecoveryAction::Retry { delay: Duration::from_secs(1), max_retries: 3 }
            }
            ErrorKind::Timeout => {
                RecoveryAction::Retry { delay: Duration::from_secs(2), max_retries: 2 }
            }
            ErrorKind::RateLimited => {
                RecoveryAction::Retry { delay: Duration::from_secs(30), max_retries: 3 }
            }
            ErrorKind::NotFound => RecoveryAction::TryAlternative {
                hint: "Tool not found. Try using a different tool.".into(),
            },
            ErrorKind::ResourceNotFound => RecoveryAction::TryAlternative {
                hint: "Resource not found. Check the path or try alternative resources.".into(),
            },
            ErrorKind::InvalidInput => RecoveryAction::TryAlternative {
                hint: "Invalid input. Review parameters and try with corrected values.".into(),
            },
            ErrorKind::PermissionDenied => RecoveryAction::ReportAndContinue {
                message: "Permission denied. Operation requires elevated privileges.".into(),
            },
            ErrorKind::ContextOverflow => RecoveryAction::TryCompression {
                hint: "Context limit exceeded. Attempting to compress context...".into(),
            },
            ErrorKind::ExecutionError | ErrorKind::Unknown => {
                if self.consecutive_failures >= 2 {
                    RecoveryAction::ReportAndContinue {
                        message: format!("Multiple execution errors: {output}"),
                    }
                } else {
                    RecoveryAction::TryAlternative {
                        hint: "Execution failed. Try a different approach.".into(),
                    }
                }
            }
        }
    }

    /// Suggest a recovery action
    fn suggest_recovery(kind: ErrorKind, output: &str) -> Option<String> {
        match kind {
            ErrorKind::NotFound => Some("The tool was not found. Check available tools.".into()),
            ErrorKind::ResourceNotFound => {
                if output.contains("file") {
                    Some("File not found. Verify the path exists.".into())
                } else {
                    Some("Resource not found. Check the path or identifier.".into())
                }
            }
            ErrorKind::PermissionDenied => {
                Some("Permission denied. Check file permissions or credentials.".into())
            }
            ErrorKind::InvalidInput => {
                Some("Invalid input. Review the parameters and try again.".into())
            }
            ErrorKind::Timeout => {
                Some("Operation timed out. Consider breaking into smaller operations.".into())
            }
            ErrorKind::RateLimited => Some("Rate limited. Wait before retrying.".into()),
            ErrorKind::NetworkError => Some("Network error. Check connectivity and retry.".into()),
            ErrorKind::ContextOverflow => {
                Some("Context too large. Summarize or trim earlier messages.".into())
            }
            ErrorKind::ExecutionError | ErrorKind::Unknown => None,
        }
    }

    /// Check if a specific error pattern is repeating too often
    #[must_use]
    pub fn is_pattern_repeating(&self, output: &str, tool_name: &str, threshold: usize) -> bool {
        let kind = classify_error(output);
        let probe = ErrorSignature { kind, tool: tool_name.to_string(), file: None };
        self.error_patterns.get(&probe).is_some_and(|&count| count >= threshold)
    }

    /// Get consecutive failure count
    #[must_use]
    pub const fn consecutive_failures(&self) -> Option<usize> {
        if self.consecutive_failures > 0 {
            Some(self.consecutive_failures)
        } else {
            None
        }
    }

    /// Get total calls
    #[must_use]
    pub const fn total_calls(&self) -> usize {
        self.total_calls
    }

    /// Get total errors
    #[must_use]
    pub const fn total_errors(&self) -> usize {
        self.total_errors
    }

    /// Get error rate
    #[must_use]
    pub fn error_rate(&self) -> f64 {
        if self.total_calls == 0 {
            0.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            let rate = self.total_errors as f64 / self.total_calls as f64;
            rate
        }
    }

    /// Reset the reflector (including checkpoint-related state)
    pub fn reset(&mut self) {
        self.tool_counts.clear();
        self.error_counts.clear();
        self.error_patterns.clear();
        self.consecutive_failures = 0;
        self.total_calls = 0;
        self.total_errors = 0;
        self.has_checkpoint = false;
        self.rollback_count = 0;
    }

    /// Set whether a git checkpoint exists
    pub const fn set_has_checkpoint(&mut self, value: bool) {
        self.has_checkpoint = value;
    }

    /// Reset error counters after a rollback so the agent gets a fresh start
    pub fn reset_counters(&mut self) {
        self.consecutive_failures = 0;
        self.error_patterns.clear();
        self.error_counts.clear();
    }

    /// Record that a rollback was performed
    pub const fn record_rollback(&mut self) {
        self.rollback_count += 1;
    }

    /// Return Rollback if checkpoint is available and budget remains, otherwise Stop
    const fn stop_or_rollback(&self, reason: String) -> RecoveryAction {
        if self.has_checkpoint && self.rollback_count < self.max_rollbacks {
            RecoveryAction::Rollback { reason }
        } else {
            RecoveryAction::Stop { reason }
        }
    }

    /// Check if we should stop due to high error rate
    #[must_use]
    pub fn should_stop(&self, min_calls: usize, max_error_rate: f64) -> bool {
        self.total_calls >= min_calls && self.error_rate() > max_error_rate
    }
}

// ---------------------------------------------------------------------------
// Helpers extracted from core_loop.rs
// ---------------------------------------------------------------------------

/// Build a signature string for episodic memory deduplication.
pub(crate) fn build_episode_signature(
    tool_name: &str,
    error_kind: Option<ErrorKind>,
    output: &str,
) -> String {
    let kind = error_kind.unwrap_or(ErrorKind::Unknown);
    let head = output.lines().next().unwrap_or("").trim();
    format!("{kind:?}:{tool_name}:{head}")
}

/// If a pending episode exists, record it as a success in the episodic store.
pub(crate) async fn maybe_append_episode_success(
    store: Option<&EpisodicMemoryStore>,
    pending_episode: &mut Option<(String, String)>,
    context_fingerprint: &str,
    tokens_used: usize,
) {
    let Some(store) = store else {
        return;
    };
    let Some((signature, strategy)) = pending_episode.take() else {
        return;
    };

    let record = EpisodeRecord {
        signature,
        context_fingerprint: context_fingerprint.to_string(),
        strategy,
        success: true,
        tokens_used,
        created_at: Utc::now(),
    };
    if let Err(e) = store.append_success(record).await {
        tracing::warn!(error = %e, "Failed to append episodic memory");
    }
}

/// Handle error recovery based on the reflector's analysis.
pub(crate) async fn handle_error_recovery(
    analysis: &ReflectionResult,
    _tool_name: &str,
    reflector: &mut Reflector,
    checkpoint_manager: &mut GitCheckpointManager,
    tx: &mpsc::Sender<crate::Result<AgentEvent>>,
) -> crate::Result<Option<String>> {
    match &analysis.recovery_action {
        RecoveryAction::Retry { delay, max_retries } => {
            let _ = tx
                .send(Ok(AgentEvent::Recovery {
                    action: format!(
                        "Retrying after {}ms (max {} retries)",
                        delay.as_millis(),
                        max_retries
                    ),
                    suggestion: analysis.suggestion.clone(),
                }))
                .await;
        }
        RecoveryAction::TryAlternative { hint } => {
            let _ = tx
                .send(Ok(AgentEvent::Recovery {
                    action: "Trying alternative approach".to_string(),
                    suggestion: Some(hint.clone()),
                }))
                .await;
        }
        RecoveryAction::ReportAndContinue { message } => {
            let _ = tx
                .send(Ok(AgentEvent::Recovery {
                    action: "Continuing despite error".to_string(),
                    suggestion: Some(message.clone()),
                }))
                .await;
        }
        RecoveryAction::TryCompression { hint } => {
            let _ = tx
                .send(Ok(AgentEvent::Recovery {
                    action: "Attempting context compression".to_string(),
                    suggestion: Some(hint.clone()),
                }))
                .await;
        }
        RecoveryAction::Rollback { reason } => match checkpoint_manager.rollback().await {
            Ok(report) => {
                let _ = tx
                    .send(Ok(AgentEvent::RolledBack {
                        reason: reason.clone(),
                        files_restored: report.files_count,
                    }))
                    .await;

                reflector.reset_counters();
                reflector.record_rollback();

                return Ok(Some(format!(
                    "[System] The working tree was rolled back because: {reason}. \
                         All file changes have been reverted. \
                         Please try a completely different approach."
                )));
            }
            Err(e) => {
                tracing::error!("Rollback failed: {}", e);
                let _ =
                    tx.send(Ok(AgentEvent::Error { message: format!("Stopping: {reason}") })).await;
                return Err(AgentError::PlanningError(reason.clone()));
            }
        },
        RecoveryAction::Stop { reason } => {
            let _ = tx.send(Ok(AgentEvent::Error { message: format!("Stopping: {reason}") })).await;
            return Err(AgentError::PlanningError(reason.clone()));
        }
        RecoveryAction::Skip => {}
    }

    Ok(None)
}

/// Check loop protection constraints (max iterations, timeout, cancellation).
pub(crate) fn check_loop_protection(
    config: &LoopProtectionConfig,
    iteration: usize,
    start_time: Instant,
    cancellation: &CancellationToken,
) -> crate::Result<()> {
    if cancellation.is_cancelled() {
        return Err(AgentError::Aborted);
    }
    if iteration > config.max_iterations {
        return Err(AgentError::MaxIterations(config.max_iterations));
    }
    if start_time.elapsed().as_secs() > config.total_timeout_secs {
        return Err(AgentError::Timeout(config.total_timeout_secs));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_classification() {
        assert_eq!(classify_error("Tool not found: xyz"), ErrorKind::NotFound);
        assert_eq!(
            classify_error("File does not exist: /path/to/file"),
            ErrorKind::ResourceNotFound
        );
        assert_eq!(classify_error("Permission denied: cannot access"), ErrorKind::PermissionDenied);
        assert_eq!(classify_error("Invalid parameter: name is required"), ErrorKind::InvalidInput);
        assert_eq!(classify_error("Request timed out after 30s"), ErrorKind::Timeout);
        assert_eq!(classify_error("Rate limit exceeded, 429"), ErrorKind::RateLimited);
        assert_eq!(classify_error("Connection refused"), ErrorKind::NetworkError);
    }

    #[test]
    fn test_error_classification_localized_messages() {
        assert_eq!(classify_error("bash: 未找到命令"), ErrorKind::NotFound);
        assert_eq!(classify_error("コマンドが見つかりません"), ErrorKind::NotFound);
        assert_eq!(classify_error("权限被拒绝: 无法访问"), ErrorKind::PermissionDenied);
        assert_eq!(classify_error("アクセスが拒否されました"), ErrorKind::PermissionDenied);
        assert_eq!(classify_error("没有那个文件或目录"), ErrorKind::ResourceNotFound);
        assert_eq!(
            classify_error("そのようなファイルやディレクトリはありません"),
            ErrorKind::ResourceNotFound
        );
        assert_eq!(classify_error("连接被拒绝"), ErrorKind::NetworkError);
        assert_eq!(classify_error("名前解決に失敗"), ErrorKind::NetworkError);
        assert_eq!(classify_error("请求过于频繁"), ErrorKind::RateLimited);
        assert_eq!(classify_error("タイムアウトしました"), ErrorKind::Timeout);
    }

    #[test]
    fn test_error_kind_retryable() {
        assert!(!ErrorKind::NotFound.is_retryable());
        assert!(!ErrorKind::InvalidInput.is_retryable());
        assert!(ErrorKind::NetworkError.is_retryable());
        assert!(ErrorKind::Timeout.is_retryable());
        assert!(ErrorKind::RateLimited.is_retryable());
    }

    #[test]
    fn test_reflector_record() {
        let mut reflector = Reflector::new();

        let success = ToolResult::success("1", "output");
        reflector.record_result(&success, "read");

        assert_eq!(reflector.total_calls(), 1);
        assert_eq!(reflector.total_errors(), 0);
        assert_eq!(reflector.consecutive_failures(), None);

        let error = ToolResult::error("2", "Connection refused");
        reflector.record_result(&error, "bash");

        assert_eq!(reflector.total_calls(), 2);
        assert_eq!(reflector.total_errors(), 1);
        assert_eq!(reflector.consecutive_failures(), Some(1));
    }

    #[test]
    fn test_reflector_analyze() {
        let reflector = Reflector::new();

        let success = ToolResult::success("1", "output");
        let analysis = reflector.analyze(&success, "read");
        assert!(analysis.success);
        assert!(!analysis.should_retry);

        let error = ToolResult::error("2", "Rate limit exceeded");
        let analysis = reflector.analyze(&error, "bash");
        assert!(!analysis.success);
        assert_eq!(analysis.error_kind, Some(ErrorKind::RateLimited));
        assert!(analysis.should_retry);
        assert!(analysis.retry_delay.is_some());
    }

    #[test]
    fn test_error_rate() {
        let mut reflector = Reflector::new();

        reflector.record_result(&ToolResult::success("1", "ok"), "read");
        reflector.record_result(&ToolResult::error("2", "fail"), "bash");

        assert!((reflector.error_rate() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_consecutive_failures() {
        let mut reflector = Reflector::new();

        reflector.record_result(&ToolResult::error("1", "fail"), "bash");
        reflector.record_result(&ToolResult::error("2", "fail"), "bash");
        assert_eq!(reflector.consecutive_failures(), Some(2));

        reflector.record_result(&ToolResult::success("3", "ok"), "read");
        assert_eq!(reflector.consecutive_failures(), None);
    }

    #[test]
    fn test_recovery_actions() {
        let reflector = Reflector::new();

        // Network error should suggest retry
        let error = ToolResult::error("1", "Connection refused");
        let analysis = reflector.analyze(&error, "bash");
        match analysis.recovery_action {
            RecoveryAction::Retry { max_retries, .. } => {
                assert!(max_retries > 0);
            }
            _ => panic!("Expected Retry action for network error"),
        }

        // Rate limit should suggest retry with longer delay
        let error = ToolResult::error("2", "Rate limit exceeded");
        let analysis = reflector.analyze(&error, "bash");
        match analysis.recovery_action {
            RecoveryAction::Retry { delay, .. } => {
                assert!(delay.as_secs() >= 30);
            }
            _ => panic!("Expected Retry action for rate limit"),
        }

        // Permission denied should report and continue
        let error = ToolResult::error("3", "Permission denied: access denied");
        let analysis = reflector.analyze(&error, "bash");
        match analysis.recovery_action {
            RecoveryAction::ReportAndContinue { .. } => {}
            _ => panic!("Expected ReportAndContinue for permission denied"),
        }

        // Invalid input should suggest alternative
        let error = ToolResult::error("4", "Invalid parameter: missing required field");
        let analysis = reflector.analyze(&error, "read");
        match analysis.recovery_action {
            RecoveryAction::TryAlternative { .. } => {}
            _ => panic!("Expected TryAlternative for invalid input"),
        }

        // Context overflow should try compression
        let error = ToolResult::error("5", "Context token limit exceeded");
        let analysis = reflector.analyze(&error, "bash");
        match analysis.recovery_action {
            RecoveryAction::TryCompression { .. } => {}
            _ => panic!("Expected TryCompression for context overflow"),
        }
    }

    #[test]
    fn test_error_signature_dedup() {
        let mut reflector = Reflector::new();

        // Same tool + same error kind on different files should count together
        reflector
            .record_result(&ToolResult::error("1", "File does not exist: /src/foo.rs"), "read");
        reflector
            .record_result(&ToolResult::error("2", "File does not exist: /src/bar.rs"), "read");

        // Both are ResourceNotFound + "read" -> count should be 2
        assert!(reflector.is_pattern_repeating("File does not exist: /src/baz.rs", "read", 2));
        // Different tool should not match
        assert!(!reflector.is_pattern_repeating("File does not exist: /src/baz.rs", "bash", 1));
    }

    #[test]
    fn test_extract_file_hint() {
        assert_eq!(
            extract_file_hint("File does not exist: /src/foo.rs"),
            Some("/src/foo.rs".to_string())
        );
        assert_eq!(
            extract_file_hint("Error reading ./config.toml"),
            Some("./config.toml".to_string())
        );
        assert_eq!(extract_file_hint("Connection refused"), None);
    }

    #[test]
    fn test_rollback_instead_of_stop_when_checkpoint_exists() {
        let mut reflector = Reflector::new();
        reflector.set_has_checkpoint(true);

        // Trigger enough consecutive failures to hit Stop threshold
        for i in 0..reflector.config.max_consecutive_failures {
            reflector
                .record_result(&ToolResult::error(&format!("{i}"), "some execution error"), "bash");
        }

        let error = ToolResult::error("final", "some execution error");
        let analysis = reflector.analyze(&error, "bash");

        // Should get Rollback instead of Stop
        assert!(
            matches!(analysis.recovery_action, RecoveryAction::Rollback { .. }),
            "Expected Rollback, got {:?}",
            analysis.recovery_action
        );
    }

    #[test]
    fn test_stop_after_rollback_budget_exhausted() {
        let mut reflector = Reflector::new();
        reflector.set_has_checkpoint(true);

        // Exhaust rollback budget
        reflector.record_rollback();
        reflector.record_rollback();

        // Trigger consecutive failures
        for i in 0..reflector.config.max_consecutive_failures {
            reflector
                .record_result(&ToolResult::error(&format!("{i}"), "some execution error"), "bash");
        }

        let error = ToolResult::error("final", "some execution error");
        let analysis = reflector.analyze(&error, "bash");

        // Budget exhausted — should get Stop
        assert!(
            matches!(analysis.recovery_action, RecoveryAction::Stop { .. }),
            "Expected Stop after budget exhausted, got {:?}",
            analysis.recovery_action
        );
    }

    #[test]
    fn test_reset_clears_checkpoint_state() {
        let mut reflector = Reflector::new();
        reflector.set_has_checkpoint(true);
        reflector.record_rollback();

        reflector.reset();

        // After reset, checkpoint state should be cleared
        assert!(!reflector.has_checkpoint);
        assert_eq!(reflector.rollback_count, 0);
    }

    #[test]
    fn test_reset_counters_preserves_rollback_count() {
        let mut reflector = Reflector::new();
        reflector.record_rollback();
        reflector.record_result(&ToolResult::error("1", "fail"), "bash");

        reflector.reset_counters();

        // reset_counters clears error state but keeps rollback_count
        assert_eq!(reflector.consecutive_failures(), None);
        assert_eq!(reflector.rollback_count, 1);
    }
}
