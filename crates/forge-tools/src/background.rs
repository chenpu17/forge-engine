//! Background Task Manager
//!
//! Manages background execution of shell commands and sub-agent tasks.
//! Provides APIs to spawn, monitor, and control background tasks.

use crate::builtin::shell::get_shell_executor;
use crate::platform::{KillTreeResult, ProcessManager};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, RwLock};

/// Background task types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskType {
    /// Shell command (bash)
    Shell,
    /// Sub-agent task
    SubAgent,
}

impl std::fmt::Display for BackgroundTaskType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shell => write!(f, "shell"),
            Self::SubAgent => write!(f, "subagent"),
        }
    }
}

/// Background task status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskStatus {
    /// Task is running
    Running,
    /// Task completed successfully
    Completed {
        /// Exit code (for shell tasks)
        exit_code: Option<i32>,
    },
    /// Task failed
    Failed {
        /// Error message
        error: String,
    },
    /// Task was killed
    Killed,
}

impl BackgroundTaskStatus {
    /// Check if task is still running
    #[must_use]
    pub const fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }

    /// Check if task completed successfully
    #[must_use]
    pub const fn is_success(&self) -> bool {
        matches!(self, Self::Completed { exit_code: Some(0) | None })
    }
}

/// Output chunk from background task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputChunk {
    /// Timestamp when output was received
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Output content
    pub content: String,
    /// Whether this is stderr
    pub is_stderr: bool,
}

/// Background task instance
pub struct BackgroundTask {
    /// Unique task ID
    pub id: String,
    /// Task type
    pub task_type: BackgroundTaskType,
    /// Description
    pub description: String,
    /// Command or prompt
    pub command: String,
    /// Working directory
    pub working_dir: PathBuf,
    /// Current status
    pub status: BackgroundTaskStatus,
    /// Output buffer (accumulated)
    pub output: Vec<OutputChunk>,
    /// Total output size in bytes
    output_size: usize,
    /// Process handle (for shell tasks)
    process: Option<Child>,
    /// Cancellation sender
    cancel_tx: Option<broadcast::Sender<()>>,
    /// Started timestamp
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Completed timestamp
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl BackgroundTask {
    /// Create a new background task
    fn new(
        task_type: BackgroundTaskType,
        description: String,
        command: String,
        working_dir: PathBuf,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            task_type,
            description,
            command,
            working_dir,
            status: BackgroundTaskStatus::Running,
            output: Vec::new(),
            output_size: 0,
            process: None,
            cancel_tx: None,
            started_at: chrono::Utc::now(),
            completed_at: None,
        }
    }

    /// Add output chunk with size limit
    fn add_output(&mut self, content: String, is_stderr: bool, max_size: usize) {
        let chunk_size = content.len();
        if self.output_size + chunk_size <= max_size {
            self.output.push(OutputChunk { timestamp: chrono::Utc::now(), content, is_stderr });
            self.output_size += chunk_size;
        }
        // If over limit, just count but don't store
    }

    /// Get combined output as string
    #[must_use]
    pub fn get_output(&self) -> String {
        let mut stdout = String::new();
        let mut stderr = String::new();

        for chunk in &self.output {
            if chunk.is_stderr {
                stderr.push_str(&chunk.content);
                stderr.push('\n');
            } else {
                stdout.push_str(&chunk.content);
                stdout.push('\n');
            }
        }

        if stderr.is_empty() {
            stdout
        } else {
            format!("{stdout}\n[stderr]\n{stderr}")
        }
    }

    /// Mark task as completed
    fn complete(&mut self, exit_code: Option<i32>) {
        self.status = BackgroundTaskStatus::Completed { exit_code };
        self.completed_at = Some(chrono::Utc::now());
        self.process = None;
    }

    /// Mark task as failed
    fn fail(&mut self, error: String) {
        self.status = BackgroundTaskStatus::Failed { error };
        self.completed_at = Some(chrono::Utc::now());
        self.process = None;
    }

    /// Mark task as killed
    fn kill(&mut self) {
        self.status = BackgroundTaskStatus::Killed;
        self.completed_at = Some(chrono::Utc::now());
        self.process = None;
    }
}

/// Summary for listing tasks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundTaskSummary {
    /// Task ID
    pub id: String,
    /// Task type
    pub task_type: BackgroundTaskType,
    /// Description
    pub description: String,
    /// Current status
    pub status: BackgroundTaskStatus,
    /// Started timestamp
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Completed timestamp
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<&BackgroundTask> for BackgroundTaskSummary {
    fn from(task: &BackgroundTask) -> Self {
        Self {
            id: task.id.clone(),
            task_type: task.task_type,
            description: task.description.clone(),
            status: task.status.clone(),
            started_at: task.started_at,
            completed_at: task.completed_at,
        }
    }
}

/// Result from `get_output`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskOutputResult {
    /// Task ID
    pub id: String,
    /// Current status
    pub status: BackgroundTaskStatus,
    /// Output content
    pub output: String,
    /// Whether task is still running
    pub is_running: bool,
}

/// Global background task manager
pub struct BackgroundTaskManager {
    /// Active tasks by ID
    tasks: RwLock<HashMap<String, Arc<RwLock<BackgroundTask>>>>,
    /// Maximum concurrent background tasks
    max_tasks: usize,
    /// Output buffer limit per task (bytes)
    max_output_size: usize,
}

impl std::fmt::Debug for BackgroundTaskManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackgroundTaskManager")
            .field("max_tasks", &self.max_tasks)
            .field("max_output_size", &self.max_output_size)
            .finish_non_exhaustive()
    }
}

impl Default for BackgroundTaskManager {
    fn default() -> Self {
        Self::new()
    }
}

impl BackgroundTaskManager {
    /// Create a new background task manager with default settings
    #[must_use]
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            max_tasks: 10,
            max_output_size: 1024 * 1024, // 1MB per task
        }
    }

    /// Create with custom limits
    #[must_use]
    pub fn with_limits(max_tasks: usize, max_output_size: usize) -> Self {
        Self { tasks: RwLock::new(HashMap::new()), max_tasks, max_output_size }
    }

    /// Spawn a shell command in background
    ///
    /// Returns the task ID on success
    ///
    /// # Errors
    ///
    /// Returns an error if the maximum task limit is reached or the process fails to spawn.
    #[allow(clippy::too_many_lines)]
    pub async fn spawn_shell(
        &self,
        command: &str,
        description: &str,
        working_dir: &Path,
    ) -> Result<String, String> {
        // Check task limit - count only running tasks
        {
            let tasks = self.tasks.read().await;
            let mut running_count = 0;
            for task in tasks.values() {
                let task_guard = task.read().await;
                if task_guard.status.is_running() {
                    running_count += 1;
                }
            }
            drop(tasks);

            if running_count >= self.max_tasks {
                return Err(format!(
                    "Maximum background tasks ({}) reached. Use task_output to check existing tasks or kill_shell to terminate them.",
                    self.max_tasks
                ));
            }
        }

        // Create task
        let mut task = BackgroundTask::new(
            BackgroundTaskType::Shell,
            description.to_string(),
            command.to_string(),
            working_dir.to_path_buf(),
        );

        // Create cancellation channel
        let (cancel_tx, _) = broadcast::channel::<()>(1);
        task.cancel_tx = Some(cancel_tx.clone());

        // Spawn process using platform-specific shell executor
        let executor = get_shell_executor();
        let mut cmd = Command::new(executor.program());

        // Add extra args (e.g., -NoProfile for PowerShell)
        for arg in executor.extra_args() {
            cmd.arg(arg);
        }

        // Add command argument
        cmd.arg(executor.command_arg());
        if executor.use_encoded_command() {
            // PowerShell: use encoded command
            cmd.arg(executor.encode_command(command));
        } else {
            // Bash: use -c with raw command
            cmd.arg(command);
        }

        // Note: For proper kill_tree support on Unix, we would need to use
        // process groups (setpgid), but that requires unsafe code.
        // For now, we rely on tokio's kill() which sends SIGKILL.
        // The platform::ProcessManager provides kill_tree for more robust termination.

        let mut child = cmd
            .current_dir(working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn process: {e}"))?;

        let stdout = child.stdout.take().ok_or_else(|| "Failed to capture stdout".to_string())?;
        let stderr = child.stderr.take().ok_or_else(|| "Failed to capture stderr".to_string())?;

        task.process = Some(child);
        let task_id = task.id.clone();

        // Store task
        let task_arc = Arc::new(RwLock::new(task));
        {
            let mut tasks = self.tasks.write().await;
            tasks.insert(task_id.clone(), task_arc.clone());
        }

        // Spawn background reader task
        let max_output = self.max_output_size;
        let mut cancel_rx = cancel_tx.subscribe();

        tokio::spawn(async move {
            let mut stdout_reader = BufReader::new(stdout).lines();
            let mut stderr_reader = BufReader::new(stderr).lines();

            loop {
                tokio::select! {
                    // Check for cancellation
                    _ = cancel_rx.recv() => {
                        let mut task = task_arc.write().await;
                        if let Some(mut process) = task.process.take() {
                            let _ = process.kill().await;
                        }
                        task.kill();
                        drop(task);
                        break;
                    }

                    // Read stdout
                    line = stdout_reader.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                let mut task = task_arc.write().await;
                                task.add_output(line, false, max_output);
                            }
                            Ok(None) => {
                                // stdout closed, wait for process to finish
                                let mut task = task_arc.write().await;
                                if let Some(mut process) = task.process.take() {
                                    match process.wait().await {
                                        Ok(status) => {
                                            task.complete(status.code());
                                        }
                                        Err(e) => {
                                            task.fail(format!("Process wait error: {e}"));
                                        }
                                    }
                                }
                                drop(task);
                                break;
                            }
                            Err(e) => {
                                task_arc.write().await.fail(format!("Read error: {e}"));
                                break;
                            }
                        }
                    }

                    // Read stderr
                    line = stderr_reader.next_line() => {
                        if let Ok(Some(line)) = line {
                            let mut task = task_arc.write().await;
                            task.add_output(line, true, max_output);
                        }
                    }
                }
            }
        });

        Ok(task_id)
    }

    /// Get task by ID
    pub async fn get_task(&self, id: &str) -> Option<Arc<RwLock<BackgroundTask>>> {
        let tasks = self.tasks.read().await;
        tasks.get(id).cloned()
    }

    /// Get task output
    ///
    /// If `wait` is true, blocks until task completes or timeout
    ///
    /// # Errors
    ///
    /// Returns an error if the task is not found.
    pub async fn get_output(
        &self,
        id: &str,
        wait: bool,
        timeout_ms: Option<u64>,
    ) -> Result<TaskOutputResult, String> {
        let task_arc = self.get_task(id).await.ok_or_else(|| format!("Task not found: {id}"))?;

        if wait {
            let timeout = Duration::from_millis(timeout_ms.unwrap_or(30_000));
            let start = std::time::Instant::now();

            loop {
                {
                    let task = task_arc.read().await;
                    if !task.status.is_running() {
                        return Ok(TaskOutputResult {
                            id: task.id.clone(),
                            status: task.status.clone(),
                            output: task.get_output(),
                            is_running: false,
                        });
                    }
                }

                if start.elapsed() >= timeout {
                    let task = task_arc.read().await;
                    return Ok(TaskOutputResult {
                        id: task.id.clone(),
                        status: task.status.clone(),
                        output: task.get_output(),
                        is_running: true,
                    });
                }

                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        } else {
            let task = task_arc.read().await;
            Ok(TaskOutputResult {
                id: task.id.clone(),
                status: task.status.clone(),
                output: task.get_output(),
                is_running: task.status.is_running(),
            })
        }
    }

    /// Kill a background task
    ///
    /// If `force` is true, uses force kill (SIGKILL on Unix, immediate taskkill on Windows).
    /// Otherwise uses graceful termination (SIGTERM on Unix, taskkill on Windows).
    /// Uses `ProcessManager::kill_tree` to terminate the entire process tree.
    ///
    /// # Errors
    ///
    /// Returns an error if the task is not found, not running, or kill fails.
    pub async fn kill(&self, id: &str, force: bool) -> Result<(), String> {
        let task_arc = self.get_task(id).await.ok_or_else(|| format!("Task not found: {id}"))?;

        let mut task = task_arc.write().await;

        if !task.status.is_running() {
            return Err(format!("Task {id} is not running"));
        }

        // Send cancellation signal
        if let Some(cancel_tx) = &task.cancel_tx {
            let _ = cancel_tx.send(());
        }

        // Use ProcessManager to kill the process tree
        if let Some(mut process) = task.process.take() {
            if let Some(pid) = process.id() {
                let result = if force {
                    ProcessManager::force_kill(pid).await
                } else {
                    ProcessManager::kill_tree(pid).await
                };

                match result {
                    KillTreeResult::Success => {}
                    KillTreeResult::PermissionDenied(msg) => {
                        return Err(format!("Permission denied: {msg}"));
                    }
                    KillTreeResult::Error(msg) => {
                        // Fall back to tokio's kill as last resort
                        tracing::warn!(
                            "ProcessManager::kill_tree failed: {}, falling back to tokio kill",
                            msg
                        );
                        process.kill().await.map_err(|e| format!("Kill failed: {e}"))?;
                    }
                }
            } else {
                // No PID available, use tokio's kill directly
                process.kill().await.map_err(|e| format!("Kill failed: {e}"))?;
            }
        }
        task.kill();
        drop(task);

        Ok(())
    }

    /// List all tasks
    pub async fn list_tasks(&self) -> Vec<BackgroundTaskSummary> {
        let tasks = self.tasks.read().await;
        let mut summaries = Vec::new();

        for task_arc in tasks.values() {
            let task = task_arc.read().await;
            summaries.push(BackgroundTaskSummary::from(&*task));
        }
        drop(tasks);

        // Sort by start time, newest first
        summaries.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        summaries
    }

    /// Cleanup completed tasks older than duration
    pub async fn cleanup(&self, max_age: Duration) {
        let now = chrono::Utc::now();
        let mut to_remove = Vec::new();

        {
            let tasks = self.tasks.read().await;
            for (id, task_arc) in tasks.iter() {
                let task = task_arc.read().await;
                if let Some(completed_at) = task.completed_at {
                    let age = now.signed_duration_since(completed_at);
                    #[allow(clippy::cast_possible_wrap)]
                    if age.num_seconds() > max_age.as_secs() as i64 {
                        to_remove.push(id.clone());
                    }
                }
            }
        }

        if !to_remove.is_empty() {
            let mut tasks = self.tasks.write().await;
            for id in to_remove {
                tasks.remove(&id);
            }
        }
    }

    /// Get count of running tasks
    pub async fn running_count(&self) -> usize {
        let tasks = self.tasks.read().await;
        let mut count = 0;
        for task_arc in tasks.values() {
            let task = task_arc.read().await;
            if task.status.is_running() {
                count += 1;
            }
        }
        drop(tasks);
        count
    }

    /// Get count of running subagent tasks
    pub async fn subagent_count(&self) -> usize {
        let tasks = self.tasks.read().await;
        let mut count = 0;
        for task_arc in tasks.values() {
            let task = task_arc.read().await;
            if task.task_type == BackgroundTaskType::SubAgent && task.status.is_running() {
                count += 1;
            }
        }
        drop(tasks);
        count
    }

    /// Spawn a subagent task in background
    ///
    /// Returns the task ID on success. The executor future will be spawned
    /// and run asynchronously.
    ///
    /// # Errors
    ///
    /// Returns an error if the maximum concurrent subagent limit is reached.
    pub async fn spawn_subagent<F>(
        &self,
        task_id: String,
        description: &str,
        prompt: &str,
        working_dir: &Path,
        max_concurrent: usize,
        executor: F,
    ) -> Result<String, String>
    where
        F: std::future::Future<Output = Result<String, String>> + Send + 'static,
    {
        // Check subagent-specific concurrent limit
        let current_count = self.subagent_count().await;
        if current_count >= max_concurrent {
            return Err(format!(
                "Maximum concurrent subagent tasks ({max_concurrent}) reached. \
                 Use task_output to check existing tasks."
            ));
        }

        // Create task
        let mut task = BackgroundTask::new(
            BackgroundTaskType::SubAgent,
            description.to_string(),
            prompt.to_string(),
            working_dir.to_path_buf(),
        );
        task.id = task_id.clone();

        // Create cancellation channel
        let (cancel_tx, mut cancel_rx) = broadcast::channel::<()>(1);
        task.cancel_tx = Some(cancel_tx);

        // Store task
        let task_arc = Arc::new(RwLock::new(task));
        {
            let mut tasks = self.tasks.write().await;
            tasks.insert(task_id.clone(), task_arc.clone());
        }

        let max_output = self.max_output_size;

        // Spawn background executor task
        tokio::spawn(async move {
            tokio::select! {
                // Check for cancellation
                _ = cancel_rx.recv() => {
                    let mut task = task_arc.write().await;
                    task.kill();
                }

                // Execute the subagent
                result = executor => {
                    let mut task = task_arc.write().await;
                    match result {
                        Ok(output) => {
                            // Store output (truncate if too large)
                            let truncated = if output.len() > max_output {
                                format!(
                                    "{}...\n[Output truncated, {} bytes total]",
                                    &output[..max_output],
                                    output.len()
                                )
                            } else {
                                output
                            };
                            task.add_output(truncated, false, max_output);
                            task.complete(Some(0));
                        }
                        Err(error) => {
                            task.add_output(error.clone(), true, max_output);
                            task.fail(error);
                        }
                    }
                }
            }
        });

        Ok(task_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_spawn_shell_basic() {
        let manager = BackgroundTaskManager::new();
        let dir = tempdir().unwrap();

        let task_id = manager.spawn_shell("echo hello", "Test echo", dir.path()).await.unwrap();

        assert!(!task_id.is_empty());

        // Wait for completion
        tokio::time::sleep(Duration::from_millis(500)).await;

        let result = manager.get_output(&task_id, false, None).await.unwrap();
        assert!(!result.is_running);
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn test_spawn_shell_with_wait() {
        let manager = BackgroundTaskManager::new();
        let dir = tempdir().unwrap();

        // Use platform-appropriate sleep + echo command
        let cmd = if cfg!(windows) {
            "Start-Sleep -Milliseconds 100; Write-Output done"
        } else {
            "sleep 0.1 && echo done"
        };

        let task_id = manager.spawn_shell(cmd, "Sleep test", dir.path()).await.unwrap();

        let result = manager.get_output(&task_id, true, Some(5000)).await.unwrap();

        assert!(!result.is_running);
        assert!(result.output.contains("done"));
    }

    #[tokio::test]
    async fn test_kill_shell() {
        let manager = BackgroundTaskManager::new();
        let dir = tempdir().unwrap();

        // Use platform-appropriate long sleep command
        let cmd = if cfg!(windows) { "Start-Sleep -Seconds 10" } else { "sleep 10" };

        let task_id = manager.spawn_shell(cmd, "Long sleep", dir.path()).await.unwrap();

        // Give it a moment to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Kill it
        manager.kill(&task_id, true).await.unwrap();

        // Check status
        tokio::time::sleep(Duration::from_millis(100)).await;
        let result = manager.get_output(&task_id, false, None).await.unwrap();
        assert!(!result.is_running);
        assert_eq!(result.status, BackgroundTaskStatus::Killed);
    }

    #[tokio::test]
    async fn test_list_tasks() {
        let manager = BackgroundTaskManager::new();
        let dir = tempdir().unwrap();

        let _id1 = manager.spawn_shell("echo task1", "Task 1", dir.path()).await.unwrap();
        let _id2 = manager.spawn_shell("echo task2", "Task 2", dir.path()).await.unwrap();

        let tasks = manager.list_tasks().await;
        assert_eq!(tasks.len(), 2);
    }

    #[tokio::test]
    async fn test_task_not_found() {
        let manager = BackgroundTaskManager::new();
        let result = manager.get_output("nonexistent", false, None).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_task_status() {
        assert!(BackgroundTaskStatus::Running.is_running());
        assert!(!BackgroundTaskStatus::Completed { exit_code: Some(0) }.is_running());
        assert!(BackgroundTaskStatus::Completed { exit_code: Some(0) }.is_success());
        assert!(!BackgroundTaskStatus::Completed { exit_code: Some(1) }.is_success());
    }
}
