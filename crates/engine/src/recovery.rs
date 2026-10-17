//! Local desktop recovery. Encoding and file I/O run on a worker, outside typing.
use crate::{CmdError, CmdResult, Session};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryInfo {
    pub id: String,
    pub name: String,
    pub saved_at: String,
    /// Pictures and other parts (by part name) this copy doesn't keep: over the size budget when
    /// it was written, or found missing or damaged when it was restored (`docs/recovery.md`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub omitted: Vec<String>,
}

/// What the current document's recovery copy doesn't cover, shown as "Not protected by
/// recovery" until a later copy covers it all or the document is saved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecoveryGap {
    /// The copy keeps the text and structure but not these pictures or parts (the size budget).
    Omitted(Vec<String>),
    /// No copy could be written, for this reason.
    Failed(String),
}

#[cfg(not(target_arch = "wasm32"))]
mod desktop;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use desktop::Recovery;

impl Session {
    /// The desktop host supplies the profile folder and a thread-safe wakeup.
    /// Headless sessions and the browser never write recovery files implicitly.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn enable_recovery(&mut self, root: std::path::PathBuf, wake: std::sync::Arc<dyn Fn() + Send + Sync>) -> Result<(), String> {
        self.recovery = Some(Recovery::start(root, wake)?);
        Ok(())
    }
    pub fn recovery_documents(&self) -> Vec<RecoveryInfo> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(r) = &self.recovery {
            return r.documents.clone();
        }
        Vec::new()
    }
    pub fn recovery_ready(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(r) = &self.recovery {
            return r.ready;
        }
        false
    }
    pub fn recovery_busy(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(r) = &self.recovery {
            return r.busy;
        }
        false
    }
    pub fn recovery_error(&self) -> Option<&str> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(r) = &self.recovery {
            return r.error.as_deref();
        }
        None
    }
    /// A delayed restore can't replace edits made while it was loading. Restores have no path.
    pub fn recovery_tick(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(mut r) = self.recovery.take() else { return };
            if let Some((snapshot, expected)) = r.poll() {
                if expected == (self.document_id(), self.rev()) && self.password.is_none() {
                    let id = snapshot.info.id;
                    let omitted = snapshot.info.omitted;
                    self.set_document(snapshot.document);
                    self.path = None;
                    self.dirty = true;
                    r.adopt(id, self.document_id(), self.rev());
                    if !omitted.is_empty() {
                        r.error = Some(format!("Restored without pictures or parts the recovery copy didn't keep: {}", omitted.join(", ")));
                    }
                } else {
                    r.error = Some("Recovery cancelled: the document changed while the copy was loading.".into());
                }
            }
            if let Err(e) = r.tick(self, std::time::Instant::now()) {
                r.error = Some(e);
            }
            if let Some(e) = &r.error {
                self.status = e.clone();
            }
            self.recovery = Some(r);
        }
    }
    pub fn recovery_wait(&self) -> Option<std::time::Duration> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(r) = &self.recovery
            && !r.busy
            && ((self.dirty && self.password.is_none()) || r.has_stale())
        {
            return r.wait(self.document_id(), self.rev());
        }
        None
    }
    /// What recovery doesn't cover of the current document's unsaved changes (`None`: all of
    /// them, or nothing to cover). The desktop status bar shows it.
    pub fn recovery_gap(&self) -> Option<RecoveryGap> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(r) = &self.recovery
            && self.dirty
            && self.password.is_none()
        {
            if !r.alive() {
                return Some(RecoveryGap::Failed(r.error.clone().unwrap_or_else(|| "recovery worker is unavailable".into())));
            }
            if r.document_is(self.document_id()) {
                return r.gap().cloned();
            }
        }
        None
    }
    /// Tests: the largest a recovery record (its text, pictures and parts) may be, to reach the
    /// budget without writing gigabytes.
    #[doc(hidden)]
    pub fn set_recovery_record_budget(&mut self, bytes: u64) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(r) = &mut self.recovery {
            r.limits.record = bytes;
        }
        #[cfg(target_arch = "wasm32")]
        let _ = bytes;
    }
    /// Tests: take the next snapshot at the next tick rather than after the interval.
    #[doc(hidden)]
    pub fn recovery_snapshot_due_now(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(r) = &mut self.recovery {
            r.due_now();
        }
    }
    /// Remove the current document's recovery copy now, waiting for the worker: the user chose
    /// Don't Save, or is closing a saved document. A copy left behind would be offered as unsaved
    /// work at every later start. A failure is logged and the copy stays (it is never lost).
    pub fn discard_recovery(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let document = self.document_id();
            if let Some(r) = &mut self.recovery {
                if r.document_is(document)
                    && let Err(e) = r.purge()
                {
                    log::warn!("recovery: cannot discard the copy: {e}");
                }
                r.purge_stale();
            }
        }
    }
    /// Only an explicit protect/save action waits for the worker and removes plaintext copies.
    /// A failure prevents applying the password; typing never waits.
    pub(crate) fn clear_recovery_before_password(&mut self) -> Result<(), CmdError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let document = self.document_id();
            if let Some(r) = &mut self.recovery
                && r.document_is(document)
            {
                r.purge().map_err(CmdError::Failed)?;
            }
        }
        Ok(())
    }
    pub(crate) fn recover(&mut self, v: &Value) -> CmdResult {
        let restore = v.get("restore");
        let discard = v.get("discard");
        if restore.is_some() && discard.is_some() {
            return Err(CmdError::Params("choose restore or discard".into()));
        }
        if restore.is_some() || discard.is_some() || v.get("refresh").is_some() {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let expected = (self.document_id(), self.rev());
                let r = self.recovery.as_mut().ok_or_else(|| CmdError::Disabled("local recovery requires the desktop host".into()))?;
                if let Some(id) = restore {
                    r.restore(id.as_str().ok_or_else(|| CmdError::Params("restore must be a recovery id".into()))?, expected)
                        .map_err(CmdError::Failed)?;
                } else if let Some(id) = discard {
                    r.discard(id.as_str().ok_or_else(|| CmdError::Params("discard must be a recovery id".into()))?, expected)
                        .map_err(CmdError::Failed)?;
                } else if v.get("refresh").and_then(Value::as_bool) == Some(true) {
                    r.refresh().map_err(CmdError::Failed)?;
                } else {
                    return Err(CmdError::Params("refresh must be true".into()));
                }
            }
            #[cfg(target_arch = "wasm32")]
            return Err(CmdError::Disabled("local recovery is available on desktop only".into()));
        }
        Ok(
            json!({"enabled": self.recovery_ready(), "busy": self.recovery_busy(), "documents": self.recovery_documents(), "error": self.recovery_error(), "gap": self.recovery_gap()}),
        )
    }
}
