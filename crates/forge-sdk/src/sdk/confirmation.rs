//! Confirmation handling and plan mode management for ForgeSDK

use super::*;

impl ForgeSDK {
    // ========================
    // Confirmation API
    // ========================

    /// Respond to a pending tool confirmation request.
    ///
    /// # Errors
    ///
    /// Returns error if confirmation ID not found or ambiguous.
    pub async fn respond_to_confirmation(&self, id: &str, allowed: bool) -> Result<()> {
        let mut pending = self.pending_confirmations.write().await;
        let matches: Vec<ConfirmationKey> =
            pending.keys().filter(|k| k.confirmation_id == id).cloned().collect();

        if matches.is_empty() {
            return Err(ForgeError::InvalidConfirmation(id.to_string()));
        }
        if matches.len() > 1 {
            return Err(ForgeError::InvalidConfirmation(format!(
                "Ambiguous confirmation id '{id}' ({} matches)",
                matches.len()
            )));
        }

        let key = matches[0].clone();
        if let Some(confirmation) = pending.remove(&key) {
            let _ = confirmation.response_tx.send(allowed);
            Ok(())
        } else {
            Err(ForgeError::InvalidConfirmation(id.to_string()))
        }
    }

    /// Check if there are any pending confirmations.
    pub async fn has_pending_confirmations(&self) -> bool {
        !self.pending_confirmations.read().await.is_empty()
    }

    /// Check if there are pending confirmations for a specific session.
    pub async fn has_pending_confirmations_in_session(&self, session_id: &str) -> bool {
        self.pending_confirmations
            .read()
            .await
            .keys()
            .any(|k| k.session_id == session_id)
    }

    /// Respond to a confirmation routed by session id.
    ///
    /// # Errors
    ///
    /// Returns error if confirmation not found or ambiguous.
    pub async fn respond_to_confirmation_in_session(
        &self,
        session_id: &str,
        confirmation_id: &str,
        allowed: bool,
        _always_allow: bool,
    ) -> Result<()> {
        let mut pending = self.pending_confirmations.write().await;
        let matches: Vec<ConfirmationKey> = pending
            .keys()
            .filter(|k| k.session_id == session_id && k.confirmation_id == confirmation_id)
            .cloned()
            .collect();

        if matches.is_empty() {
            return Err(ForgeError::InvalidConfirmation(format!(
                "{session_id}:{confirmation_id}"
            )));
        }
        if matches.len() > 1 {
            return Err(ForgeError::InvalidConfirmation(format!(
                "Ambiguous confirmation id '{confirmation_id}' in session '{session_id}' ({} matches)",
                matches.len()
            )));
        }

        let key = matches[0].clone();
        if let Some(confirmation) = pending.remove(&key) {
            let _ = confirmation.response_tx.send(allowed);
            Ok(())
        } else {
            Err(ForgeError::InvalidConfirmation(format!(
                "{session_id}:{confirmation_id}"
            )))
        }
    }

    /// Respond to a confirmation routed by session + request id.
    ///
    /// # Errors
    ///
    /// Returns error if confirmation not found.
    pub async fn respond_to_confirmation_in_request(
        &self,
        session_id: &str,
        request_id: &str,
        confirmation_id: &str,
        allowed: bool,
    ) -> Result<()> {
        let mut pending = self.pending_confirmations.write().await;
        let key = ConfirmationKey {
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            confirmation_id: confirmation_id.to_string(),
        };
        if let Some(confirmation) = pending.remove(&key) {
            let _ = confirmation.response_tx.send(allowed);
            Ok(())
        } else {
            Err(ForgeError::InvalidConfirmation(format!(
                "{session_id}:{request_id}:{confirmation_id}"
            )))
        }
    }

    /// Cancel all pending confirmations.
    pub async fn cancel_all_confirmations(&self) {
        let mut pending = self.pending_confirmations.write().await;
        for (_, confirmation) in pending.drain() {
            let _ = confirmation.response_tx.send(false);
        }
    }

    /// Cancel all pending confirmations for a specific session.
    pub async fn cancel_all_confirmations_in_session(&self, session_id: &str) {
        let mut pending = self.pending_confirmations.write().await;
        let keys: Vec<ConfirmationKey> = pending
            .keys()
            .filter(|k| k.session_id == session_id)
            .cloned()
            .collect();
        for key in keys {
            if let Some(confirmation) = pending.remove(&key) {
                let _ = confirmation.response_tx.send(false);
            }
        }
    }

    // ========================
    // Path Confirmation API
    // ========================

    /// Add a confirmed path that can be accessed without prompting.
    pub async fn add_confirmed_path(&self, path: PathBuf) {
        let canonical = path.canonicalize().unwrap_or(path);
        self.confirmed_paths.write().await.insert(canonical);
    }

    /// Replace the confirmed path set.
    pub async fn set_confirmed_paths(&self, paths: Vec<PathBuf>) {
        let mut confirmed: HashSet<PathBuf> = HashSet::new();
        for path in paths {
            confirmed.insert(path.canonicalize().unwrap_or(path));
        }
        *self.confirmed_paths.write().await = confirmed;
    }

    /// Clear all confirmed paths.
    pub async fn clear_confirmed_paths(&self) {
        self.confirmed_paths.write().await.clear();
    }

    // ========================
    // Plan Mode API
    // ========================

    /// Enter plan mode (write operations disabled).
    pub async fn enter_plan_mode(&self, plan_file: Option<PathBuf>) {
        self.plan_mode_flag.store(true, Ordering::Release);
        *self.plan_file_path.write().await = plan_file;
    }

    /// Exit plan mode (re-enables all tools).
    pub async fn exit_plan_mode(&self) {
        self.plan_mode_flag.store(false, Ordering::Release);
        *self.plan_file_path.write().await = None;
    }

    /// Check if plan mode is active.
    pub fn is_plan_mode_active(&self) -> bool {
        self.plan_mode_flag.load(Ordering::Acquire)
    }

    /// Get the current plan file path.
    pub async fn plan_file_path(&self) -> Option<PathBuf> {
        self.plan_file_path.read().await.clone()
    }
}
