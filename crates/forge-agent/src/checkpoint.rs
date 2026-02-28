//! Git checkpoint manager for workspace rollback
//!
//! Creates a snapshot of the working tree before the first write operation
//! and can restore it when the Reflector determines the agent is stuck.

use crate::{AgentConfig, AgentError, Result};
use chrono::{DateTime, Utc};
use forge_domain::ToolCall;
use forge_llm::ChatMessage;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio::process::Command;

/// A snapshot of the git working tree state
#[derive(Debug, Clone)]
pub struct GitCheckpoint {
    /// HEAD commit SHA at checkpoint time
    pub head_sha: String,
    /// Stash object SHA (None if working tree was clean)
    pub stash_sha: Option<String>,
    /// When the checkpoint was created
    pub created_at: Instant,
}

/// Report returned after a successful rollback
#[derive(Debug, Clone)]
pub struct RollbackReport {
    /// Number of files restored
    pub files_count: usize,
    /// Whether stash was applied
    pub stash_applied: bool,
}

/// Tool execution state persisted in runtime checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeToolCallState {
    /// Unique identifier for the tool call.
    pub id: String,
    /// Name of the tool that was called.
    pub name: String,
    /// Current execution status (e.g. "pending", "completed", "failed").
    pub status: String,
}

/// Durable runtime checkpoint schema v2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeCheckpointV2 {
    /// Schema version number.
    pub version: u8,
    /// Session identifier this checkpoint belongs to.
    pub session_id: String,
    /// Current agent loop round number.
    pub round: usize,
    /// Current stage within the agent loop.
    pub stage: String,
    /// Conversation messages accumulated so far.
    pub messages: Vec<ChatMessage>,
    /// Execution state of each tool call in this round.
    pub tool_call_states: Vec<RuntimeToolCallState>,
    /// Tool call IDs awaiting user approval.
    pub pending_approvals: Vec<String>,
    /// Optional hint for the rollback strategy.
    pub rollback_hint: Option<String>,
    /// IDs of tool calls that have been applied.
    pub applied_tool_call_ids: Vec<String>,
    /// Markers for side effects that cannot be undone.
    pub side_effect_markers: Vec<String>,
    /// Timestamp of the last update.
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyRuntimeCheckpointV1 {
    pub session_id: String,
    pub round: usize,
    pub stage: String,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub tool_call_states: Vec<RuntimeToolCallState>,
    #[serde(default)]
    pub pending_approvals: Vec<String>,
    #[serde(default)]
    pub rollback_hint: Option<String>,
    pub updated_at: DateTime<Utc>,
}

/// File-backed store for durable runtime checkpoints.
#[derive(Debug, Clone)]
pub struct RuntimeCheckpointStore {
    root: PathBuf,
}

impl RuntimeCheckpointStore {
    /// Create a new store rooted under `working_dir/.forge/checkpoints`.
    #[must_use]
    pub fn new(working_dir: &Path) -> Self {
        Self { root: working_dir.join(".forge").join("checkpoints") }
    }

    fn path_for(&self, session_id: &str) -> Result<PathBuf> {
        // Allowlist: non-empty, alphanumeric with hyphens/underscores only
        if session_id.is_empty()
            || session_id.contains('\0')
            || !session_id
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return Err(AgentError::SessionError(format!(
                "Invalid session_id '{session_id}': must be non-empty alphanumeric with hyphens/underscores only"
            )));
        }
        Ok(self.root.join(format!("{session_id}.runtime-checkpoint.json")))
    }

    /// Persist a checkpoint to disk atomically (write-then-rename).
    ///
    /// # Errors
    ///
    /// Returns an error if the checkpoint directory cannot be created,
    /// serialization fails, or the file write/rename fails.
    pub async fn save(&self, checkpoint: &RuntimeCheckpointV2) -> Result<()> {
        tokio::fs::create_dir_all(&self.root).await.map_err(|e| {
            AgentError::SessionError(format!("Failed to create runtime checkpoint dir: {e}"))
        })?;

        let path = self.path_for(&checkpoint.session_id)?;
        let tmp = path.with_extension("tmp");
        let payload = serde_json::to_vec_pretty(checkpoint).map_err(|e| {
            AgentError::SessionError(format!("Failed to serialize runtime checkpoint: {e}"))
        })?;

        tokio::fs::write(&tmp, payload).await.map_err(|e| {
            AgentError::SessionError(format!("Failed to write runtime checkpoint temp file: {e}"))
        })?;
        tokio::fs::rename(&tmp, &path).await.map_err(|e| {
            AgentError::SessionError(format!("Failed to commit runtime checkpoint file: {e}"))
        })?;
        Ok(())
    }

    /// Load a checkpoint for the given session, migrating from v1 if needed.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be read or parsed.
    pub async fn load(&self, session_id: &str) -> Result<Option<RuntimeCheckpointV2>> {
        let path = self.path_for(session_id)?;
        let raw = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(AgentError::SessionError(format!(
                    "Failed to read runtime checkpoint: {e}"
                )));
            }
        };

        let value: serde_json::Value = serde_json::from_slice(&raw).map_err(|e| {
            AgentError::SessionError(format!("Failed to parse runtime checkpoint JSON: {e}"))
        })?;

        let version = value.get("version").and_then(serde_json::Value::as_u64).unwrap_or(1);
        if version == 2 {
            let cp = serde_json::from_value::<RuntimeCheckpointV2>(value).map_err(|e| {
                AgentError::SessionError(format!("Failed to decode runtime checkpoint v2: {e}"))
            })?;
            return Ok(Some(cp));
        }

        let v1 = serde_json::from_value::<LegacyRuntimeCheckpointV1>(value).map_err(|e| {
            AgentError::SessionError(format!("Failed to decode runtime checkpoint v1: {e}"))
        })?;
        Ok(Some(RuntimeCheckpointV2 {
            version: 2,
            session_id: v1.session_id,
            round: v1.round,
            stage: v1.stage,
            messages: v1.messages,
            tool_call_states: v1.tool_call_states,
            pending_approvals: v1.pending_approvals,
            rollback_hint: v1.rollback_hint,
            applied_tool_call_ids: Vec::new(),
            side_effect_markers: Vec::new(),
            updated_at: v1.updated_at,
        }))
    }

    /// Remove the checkpoint file for the given session.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be deleted.
    pub async fn clear(&self, session_id: &str) -> Result<()> {
        let path = self.path_for(session_id)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => {
                Err(AgentError::SessionError(format!("Failed to remove runtime checkpoint: {e}")))
            }
        }
    }
}

/// Manages git checkpoints for workspace rollback
#[derive(Debug)]
pub struct GitCheckpointManager {
    working_dir: PathBuf,
    current: Option<GitCheckpoint>,
}

impl GitCheckpointManager {
    /// Create a new checkpoint manager for the given working directory
    #[must_use]
    pub fn new(working_dir: &Path) -> Self {
        Self { working_dir: working_dir.to_path_buf(), current: None }
    }

    /// Whether a checkpoint currently exists
    #[must_use]
    pub const fn has_checkpoint(&self) -> bool {
        self.current.is_some()
    }

    /// Clear the current checkpoint (call on successful task completion)
    pub fn clear(&mut self) {
        self.current = None;
    }

    /// Create a checkpoint by snapshotting the current working tree.
    ///
    /// Runs `git rev-parse HEAD` and `git stash create` to capture state.
    /// If the directory is not a git repo, returns `Ok(None)` silently.
    ///
    /// # Errors
    ///
    /// Returns an error if git commands fail unexpectedly.
    pub async fn create(&mut self) -> Result<Option<&GitCheckpoint>> {
        // Check if this is a git repo
        if !self.working_dir.join(".git").exists() {
            tracing::debug!("Not a git repo, skipping checkpoint creation");
            return Ok(None);
        }

        // Get current HEAD SHA
        let head_sha = match run_git(&self.working_dir, &["rev-parse", "HEAD"]).await {
            Ok(sha) => sha,
            Err(e) => {
                tracing::warn!("Failed to get HEAD SHA for checkpoint: {e}");
                return Ok(None);
            }
        };

        // Create a stash object without modifying the working tree
        // `git stash create` returns empty string if working tree is clean
        let stash_sha = match run_git(&self.working_dir, &["stash", "create"]).await {
            Ok(sha) if sha.is_empty() => None,
            Ok(sha) => Some(sha),
            Err(e) => {
                tracing::warn!("Failed to create stash for checkpoint: {e}");
                None
            }
        };

        let checkpoint = GitCheckpoint { head_sha, stash_sha, created_at: Instant::now() };

        tracing::info!(
            head = %checkpoint.head_sha,
            has_stash = checkpoint.stash_sha.is_some(),
            "Git checkpoint created"
        );

        self.current = Some(checkpoint);
        Ok(self.current.as_ref())
    }

    /// Roll back the working tree to the checkpoint state.
    ///
    /// Sequence:
    /// 1. `git reset <head_sha>` — reset to checkpoint HEAD (handles new commits)
    /// 2. `git checkout -- .` — restore tracked files
    /// 3. `git clean -fd` — remove untracked files
    /// 4. If stash exists: `git stash apply <sha>` — restore pre-checkpoint changes
    ///
    /// # Errors
    ///
    /// Returns an error if no checkpoint exists or if critical git commands fail.
    pub async fn rollback(&mut self) -> Result<RollbackReport> {
        let checkpoint = self
            .current
            .take()
            .ok_or_else(|| AgentError::PlanningError("No checkpoint to rollback to".to_string()))?;

        let mut files_count = 0;
        let mut stash_applied = false;

        // Count changed files before rollback for the report
        if let Ok(status) = run_git(&self.working_dir, &["status", "--porcelain"]).await {
            files_count = status.lines().filter(|l| !l.is_empty()).count();
        }

        // Check if new commits were made since checkpoint
        let current_head = match run_git(&self.working_dir, &["rev-parse", "HEAD"]).await {
            Ok(sha) => sha,
            Err(e) => {
                tracing::warn!("Failed to resolve current HEAD during rollback: {e}");
                String::new()
            }
        };
        let has_new_commits = current_head != checkpoint.head_sha;

        // Step 1: Reset index to checkpoint HEAD (or current HEAD if no new commits).
        if has_new_commits {
            tracing::info!(
                from = %current_head,
                to = %checkpoint.head_sha,
                "Resetting to checkpoint HEAD"
            );
            run_git(&self.working_dir, &["reset", "--hard", &checkpoint.head_sha]).await?;
        } else {
            run_git(&self.working_dir, &["reset", "--hard", "HEAD"]).await?;
        }

        // Step 2: Restore tracked files (critical — propagate errors)
        run_git(&self.working_dir, &["checkout", "--", "."]).await?;

        // Step 3: Remove untracked files (best-effort)
        let _ = run_git(&self.working_dir, &["clean", "-fd"]).await;

        // Step 4: Restore pre-checkpoint uncommitted changes
        if let Some(ref stash_sha) = checkpoint.stash_sha {
            match run_git(&self.working_dir, &["stash", "apply", stash_sha]).await {
                Ok(_) => {
                    stash_applied = true;
                    tracing::info!("Restored pre-checkpoint changes from stash");
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to apply stash {stash_sha}: {e}. Working tree restored to HEAD.",
                    );
                }
            }
        }

        tracing::info!(
            files_restored = files_count,
            stash_applied = stash_applied,
            "Rollback complete"
        );

        Ok(RollbackReport { files_count, stash_applied })
    }
}

// ---------------------------------------------------------------------------
// Runtime resume state & helpers (extracted from core_loop.rs)
// ---------------------------------------------------------------------------

/// Mutable state tracked across iterations for durable resume.
#[derive(Default)]
pub(crate) struct RuntimeResumeState {
    pub applied_tool_call_ids: HashSet<String>,
    pub side_effect_markers: HashSet<String>,
    pub pending_approvals: Vec<String>,
}

/// Build a list of [`RuntimeToolCallState`] entries from tool calls.
pub(crate) fn format_runtime_tool_states(
    calls: &[ToolCall],
    status: &str,
) -> Vec<RuntimeToolCallState> {
    calls
        .iter()
        .map(|call| RuntimeToolCallState {
            id: call.id.clone(),
            name: call.name.clone(),
            status: status.to_string(),
        })
        .collect()
}

/// Persist a runtime checkpoint to disk (no-op when store/session are absent).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn persist_runtime_checkpoint(
    store: Option<&RuntimeCheckpointStore>,
    session_id: Option<&str>,
    round: usize,
    stage: &str,
    messages: &[ChatMessage],
    runtime_state: &RuntimeResumeState,
    tool_call_states: &[RuntimeToolCallState],
    rollback_hint: Option<&str>,
) -> Result<()> {
    let (Some(store), Some(session_id)) = (store, session_id) else {
        return Ok(());
    };

    let checkpoint = RuntimeCheckpointV2 {
        version: 2,
        session_id: session_id.to_string(),
        round,
        stage: stage.to_string(),
        messages: messages.to_vec(),
        tool_call_states: tool_call_states.to_vec(),
        pending_approvals: runtime_state.pending_approvals.clone(),
        rollback_hint: rollback_hint.map(str::to_string),
        applied_tool_call_ids: runtime_state.applied_tool_call_ids.iter().cloned().collect(),
        side_effect_markers: runtime_state.side_effect_markers.iter().cloned().collect(),
        updated_at: Utc::now(),
    };
    store.save(&checkpoint).await
}

/// Build a deduplication marker for a tool call's side effects.
pub(crate) fn build_side_effect_marker(call: &ToolCall) -> String {
    format!(
        "{}:{}",
        call.name,
        crate::tool_dispatch::normalize_json(&call.input)
    )
}

/// Build a fingerprint for the current agent context (working dir + model).
pub(crate) fn build_context_fingerprint(config: &AgentConfig) -> String {
    format!("{}|{}", config.working_dir.display(), config.model)
}

// ---------------------------------------------------------------------------
// Git helpers
// ---------------------------------------------------------------------------

/// Run a git command in the given directory and return trimmed stdout.
async fn run_git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .await
        .map_err(|e| AgentError::PlanningError(format!("Failed to run git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AgentError::PlanningError(format!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            stderr.trim()
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_new_has_no_checkpoint() {
        let mgr = GitCheckpointManager::new(&PathBuf::from("/tmp/fake"));
        assert!(!mgr.has_checkpoint());
    }

    #[test]
    fn test_clear_resets_checkpoint() {
        let mut mgr = GitCheckpointManager::new(&PathBuf::from("/tmp/fake"));
        // Manually inject a checkpoint to test clear
        mgr.current = Some(GitCheckpoint {
            head_sha: "abc123".to_string(),
            stash_sha: None,
            created_at: Instant::now(),
        });
        assert!(mgr.has_checkpoint());

        mgr.clear();
        assert!(!mgr.has_checkpoint());
    }

    #[tokio::test]
    async fn test_create_skips_non_git_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut mgr = GitCheckpointManager::new(tmp.path());

        let result = mgr.create().await;
        assert!(result.is_ok());
        assert!(result.expect("create should succeed").is_none());
        assert!(!mgr.has_checkpoint());
    }

    /// Helper: init a git repo with one commit so HEAD exists.
    async fn init_test_repo(dir: &Path) {
        run_git(dir, &["init"]).await.expect("git init");
        run_git(dir, &["config", "user.email", "test@test.com"]).await.ok();
        run_git(dir, &["config", "user.name", "Test"]).await.ok();
        // Create an initial commit so HEAD is valid
        std::fs::write(dir.join("README.md"), "init").expect("write");
        run_git(dir, &["add", "."]).await.expect("git add");
        run_git(dir, &["commit", "-m", "init"]).await.expect("git commit");
    }

    #[tokio::test]
    async fn test_create_checkpoint_in_clean_repo() {
        let tmp = tempfile::tempdir().expect("tempdir");
        init_test_repo(tmp.path()).await;

        let mut mgr = GitCheckpointManager::new(tmp.path());
        let cp = mgr.create().await.expect("create");
        assert!(cp.is_some());
        let head_sha = cp.expect("checkpoint should exist").head_sha.clone();
        let stash_sha = mgr.current.as_ref().expect("current should exist").stash_sha.clone();

        assert!(mgr.has_checkpoint());
        assert!(!head_sha.is_empty());
        // Clean repo → no stash
        assert!(stash_sha.is_none());
    }

    #[tokio::test]
    async fn test_create_checkpoint_with_dirty_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        init_test_repo(tmp.path()).await;

        // Dirty the working tree
        std::fs::write(tmp.path().join("README.md"), "modified").expect("write");

        let mut mgr = GitCheckpointManager::new(tmp.path());
        let cp = mgr.create().await.expect("create").expect("some");
        // Dirty repo → stash should exist
        assert!(cp.stash_sha.is_some());
    }

    #[tokio::test]
    async fn test_rollback_restores_working_tree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        init_test_repo(tmp.path()).await;

        let mut mgr = GitCheckpointManager::new(tmp.path());
        mgr.create().await.expect("create");

        // Simulate agent writes
        std::fs::write(tmp.path().join("agent_file.txt"), "agent wrote this").expect("write");
        std::fs::write(tmp.path().join("README.md"), "agent modified").expect("write");

        // Rollback
        let report = mgr.rollback().await.expect("rollback");
        assert!(report.files_count > 0);
        assert!(!mgr.has_checkpoint());

        // Verify: agent_file.txt should be gone, README.md restored
        assert!(!tmp.path().join("agent_file.txt").exists());
        let content = std::fs::read_to_string(tmp.path().join("README.md")).expect("read");
        assert_eq!(content, "init");
    }

    #[tokio::test]
    async fn test_rollback_undoes_agent_commits() {
        let tmp = tempfile::tempdir().expect("tempdir");
        init_test_repo(tmp.path()).await;

        let original_head = run_git(tmp.path(), &["rev-parse", "HEAD"]).await.expect("head");

        let mut mgr = GitCheckpointManager::new(tmp.path());
        mgr.create().await.expect("create");

        // Simulate agent making a commit
        std::fs::write(tmp.path().join("new.txt"), "new").expect("write");
        run_git(tmp.path(), &["add", "."]).await.expect("add");
        run_git(tmp.path(), &["commit", "-m", "agent commit"]).await.expect("commit");

        // Rollback
        mgr.rollback().await.expect("rollback");

        // HEAD should be back to original
        let head_after = run_git(tmp.path(), &["rev-parse", "HEAD"]).await.expect("head");
        assert_eq!(head_after, original_head);
        assert!(!tmp.path().join("new.txt").exists());
    }

    #[tokio::test]
    async fn test_rollback_preserves_pre_checkpoint_changes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        init_test_repo(tmp.path()).await;

        // Pre-existing uncommitted change
        std::fs::write(tmp.path().join("README.md"), "user edit").expect("write");

        let mut mgr = GitCheckpointManager::new(tmp.path());
        mgr.create().await.expect("create");

        // Agent adds a new file
        std::fs::write(tmp.path().join("agent.txt"), "agent").expect("write");

        // Rollback
        let report = mgr.rollback().await.expect("rollback");
        assert!(report.stash_applied);

        // agent.txt gone, but user edit preserved
        assert!(!tmp.path().join("agent.txt").exists());
        let content = std::fs::read_to_string(tmp.path().join("README.md")).expect("read");
        assert_eq!(content, "user edit");
    }

    #[tokio::test]
    async fn test_rollback_without_checkpoint_errors() {
        let mut mgr = GitCheckpointManager::new(&PathBuf::from("/tmp/fake"));
        let result = mgr.rollback().await;
        assert!(result.is_err());
    }
}
