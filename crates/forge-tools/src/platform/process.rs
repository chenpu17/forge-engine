//! Platform-specific process management
//!
//! Provides:
//! - Process termination (best-effort for process trees)
//! - Process alive checking
//!
//! # Process Tree Termination
//!
//! The `kill_tree` function attempts to terminate a process and its children.
//! This is a **best-effort** operation:
//!
//! - **Windows**: Uses `taskkill /T /F` which reliably terminates the process tree
//! - **Unix**: Uses `killpg` (process group) with fallback to `kill` (single process)
//!
//! On Unix, true process tree termination requires the child process to be spawned
//! as a process group leader (via `setpgid(0, 0)`). Without this, `kill_tree` will
//! only terminate the direct process, not its children.
//!
//! For reliable process tree termination on Unix, ensure processes are spawned
//! with `pre_exec` to create a new process group.

use std::time::Duration;

#[cfg(windows)]
use tokio::process::Command;

/// Result of `kill_tree` operation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KillTreeResult {
    /// Successfully terminated (including if process didn't exist)
    Success,
    /// Permission denied
    PermissionDenied(String),
    /// Other error
    Error(String),
}

/// Platform-specific process management
pub struct ProcessManager;

impl ProcessManager {
    /// Terminate a process and all its children
    ///
    /// # Success criteria
    /// - Process tree successfully terminated → Success
    /// - Process doesn't exist (already dead) → Success
    /// - Permission denied → PermissionDenied
    /// - Other errors → Error
    #[cfg(windows)]
    pub async fn kill_tree(pid: u32) -> KillTreeResult {
        // Step 1: Check if process exists
        if !Self::is_process_alive(pid).await {
            return KillTreeResult::Success;
        }

        // Step 2: Execute taskkill /T /F
        let output = match Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .output()
            .await
        {
            Ok(out) => out,
            Err(e) => {
                return KillTreeResult::Error(format!(
                    "taskkill command failed: {}. Ensure taskkill is available in PATH.",
                    e
                ));
            }
        };

        // Step 3: Judge based on exit code (not stderr text)
        match output.status.code() {
            Some(0) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if Self::is_process_alive(pid).await {
                    KillTreeResult::Error("Process still alive after taskkill".into())
                } else {
                    KillTreeResult::Success
                }
            }
            Some(1) => {
                if !Self::is_process_alive(pid).await {
                    KillTreeResult::Success
                } else {
                    KillTreeResult::PermissionDenied("Access denied or process protected".into())
                }
            }
            Some(128) => KillTreeResult::Success,
            Some(code) => KillTreeResult::Error(format!("taskkill exited with code {}", code)),
            None => KillTreeResult::Error("taskkill terminated by signal".into()),
        }
    }

    /// Terminate a process and its children (Unix) - **best-effort**
    #[cfg(unix)]
    pub async fn kill_tree(pid: u32) -> KillTreeResult {
        use nix::errno::Errno;
        use nix::sys::signal::{kill, killpg, Signal};
        use nix::unistd::Pid;

        let pid_t = Pid::from_raw(pid.cast_signed());

        match killpg(pid_t, Signal::SIGTERM) {
            Ok(()) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if Self::is_process_alive(pid).await {
                    let _ = killpg(pid_t, Signal::SIGKILL);
                }
                KillTreeResult::Success
            }
            Err(Errno::ESRCH) => match kill(pid_t, Signal::SIGTERM) {
                Ok(()) => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    if Self::is_process_alive(pid).await {
                        let _ = kill(pid_t, Signal::SIGKILL);
                    }
                    KillTreeResult::Success
                }
                Err(Errno::ESRCH) => KillTreeResult::Success,
                Err(Errno::EPERM) => KillTreeResult::PermissionDenied("Permission denied".into()),
                Err(e) => KillTreeResult::Error(e.to_string()),
            },
            Err(Errno::EPERM) => KillTreeResult::PermissionDenied("Permission denied".into()),
            Err(e) => KillTreeResult::Error(e.to_string()),
        }
    }

    /// Force kill a process and all its children
    #[cfg(windows)]
    pub async fn force_kill(pid: u32) -> KillTreeResult {
        Self::kill_tree(pid).await
    }

    /// Force kill a process and all its children (Unix)
    #[cfg(unix)]
    #[allow(clippy::unused_async)]
    pub async fn force_kill(pid: u32) -> KillTreeResult {
        use nix::errno::Errno;
        use nix::sys::signal::{kill, killpg, Signal};
        use nix::unistd::Pid;

        let pid_t = Pid::from_raw(pid.cast_signed());

        match killpg(pid_t, Signal::SIGKILL) {
            Ok(()) => KillTreeResult::Success,
            Err(Errno::ESRCH) => match kill(pid_t, Signal::SIGKILL) {
                Ok(()) | Err(Errno::ESRCH) => KillTreeResult::Success,
                Err(Errno::EPERM) => KillTreeResult::PermissionDenied("Permission denied".into()),
                Err(e) => KillTreeResult::Error(e.to_string()),
            },
            Err(Errno::EPERM) => KillTreeResult::PermissionDenied("Permission denied".into()),
            Err(e) => KillTreeResult::Error(e.to_string()),
        }
    }

    /// Check if a process is still alive (Windows)
    #[cfg(windows)]
    pub async fn is_process_alive(pid: u32) -> bool {
        match Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid), "/NH"])
            .output()
            .await
        {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                stdout.contains(&pid.to_string())
            }
            Err(_) => true,
        }
    }

    /// Check if a process is still alive (Unix)
    #[cfg(unix)]
    #[allow(clippy::unused_async)]
    pub async fn is_process_alive(pid: u32) -> bool {
        use nix::errno::Errno;
        use nix::sys::signal::kill;
        use nix::unistd::Pid;

        match kill(Pid::from_raw(pid.cast_signed()), None) {
            Err(Errno::ESRCH) => false,
            Ok(()) | Err(_) => true,
        }
    }

    /// Gracefully terminate a process (send SIGTERM on Unix, regular kill on Windows)
    #[cfg(unix)]
    #[allow(clippy::unused_async)]
    pub async fn terminate(pid: u32) -> KillTreeResult {
        use nix::errno::Errno;
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;

        match kill(Pid::from_raw(pid.cast_signed()), Signal::SIGTERM) {
            Ok(()) | Err(Errno::ESRCH) => KillTreeResult::Success,
            Err(Errno::EPERM) => KillTreeResult::PermissionDenied("Permission denied".into()),
            Err(e) => KillTreeResult::Error(e.to_string()),
        }
    }

    /// Gracefully terminate a process (Windows - same as kill)
    #[cfg(windows)]
    pub async fn terminate(pid: u32) -> KillTreeResult {
        Self::kill_tree(pid).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_kill_nonexistent_process() {
        let result = ProcessManager::kill_tree(999999).await;
        assert_eq!(result, KillTreeResult::Success);
    }

    #[tokio::test]
    async fn test_is_process_alive_nonexistent() {
        let alive = ProcessManager::is_process_alive(999999).await;
        assert!(!alive);
    }

    #[tokio::test]
    async fn test_is_process_alive_self() {
        let pid = std::process::id();
        let alive = ProcessManager::is_process_alive(pid).await;
        assert!(alive);
    }

    #[tokio::test]
    async fn test_force_kill_nonexistent() {
        let result = ProcessManager::force_kill(999999).await;
        assert_eq!(result, KillTreeResult::Success);
    }

    #[tokio::test]
    async fn test_terminate_nonexistent() {
        let result = ProcessManager::terminate(999999).await;
        assert_eq!(result, KillTreeResult::Success);
    }

    #[tokio::test]
    async fn test_kill_tree_result_eq() {
        assert_eq!(KillTreeResult::Success, KillTreeResult::Success);
        assert_eq!(
            KillTreeResult::PermissionDenied("test".into()),
            KillTreeResult::PermissionDenied("test".into())
        );
        assert_ne!(KillTreeResult::Success, KillTreeResult::Error("err".into()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_kill_spawned_process() {
        use tokio::process::Command;

        let mut child = Command::new("sleep").arg("60").spawn().expect("Failed to spawn sleep");

        let pid = child.id().expect("No PID");

        assert!(ProcessManager::is_process_alive(pid).await);

        let result = ProcessManager::terminate(pid).await;
        assert_eq!(result, KillTreeResult::Success);

        tokio::time::sleep(Duration::from_millis(200)).await;

        let _ = child.kill().await;
    }
}
