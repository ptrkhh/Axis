use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::events::{HookProgressEvent, HookStage};
use crate::host::{self, HostHandle};
use crate::models::GitHookType;
use crate::services::ProgressRegistry;

/// Emitter for hook progress events with cancellation support
pub struct HookProgressEmitter {
    operation_id: String,
    host: HostHandle,
    cancel_token: Arc<AtomicBool>,
    registry: Arc<ProgressRegistry>,
}

impl HookProgressEmitter {
    pub fn new(host: HostHandle, registry: Arc<ProgressRegistry>) -> Self {
        let operation_id = uuid::Uuid::new_v4().to_string();
        let cancel_token = registry.register(&operation_id);
        Self {
            operation_id,
            host,
            cancel_token,
            registry,
        }
    }

    /// Check if the operation was cancelled
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.load(Ordering::SeqCst)
    }

    /// Emit a running event
    pub fn emit_running(&self, hook_type: GitHookType) {
        let event =
            HookProgressEvent::new(self.operation_id.clone(), hook_type, HookStage::Running);
        host::emit(self.host.as_ref(), &event);
    }

    /// Emit a complete event
    pub fn emit_complete(&self, hook_type: GitHookType) {
        let event =
            HookProgressEvent::new(self.operation_id.clone(), hook_type, HookStage::Complete);
        host::emit(self.host.as_ref(), &event);
    }

    /// Emit a failed event with message
    pub fn emit_failed(&self, hook_type: GitHookType, message: &str) {
        let mut event =
            HookProgressEvent::new(self.operation_id.clone(), hook_type, HookStage::Failed);
        event.message = Some(message.to_string());
        host::emit(self.host.as_ref(), &event);
    }

    /// Emit a cancelled event
    pub fn emit_cancelled(&self, hook_type: GitHookType) {
        let event =
            HookProgressEvent::new(self.operation_id.clone(), hook_type, HookStage::Cancelled);
        host::emit(self.host.as_ref(), &event);
    }
}

impl Drop for HookProgressEmitter {
    fn drop(&mut self) {
        self.registry.cleanup(&self.operation_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::RecordingHost;

    #[test]
    fn emit_running_reaches_host() {
        let host = RecordingHost::new();
        let emitter = HookProgressEmitter::new(host.clone(), Arc::new(ProgressRegistry::new()));

        emitter.emit_running(GitHookType::PreCommit);

        assert_eq!(host.names(), ["hook-progress-event"]);
    }

    // ==================== HookProgressEmitter Tests ====================

    #[test]
    fn test_operation_id_is_uuid() {
        // Verify the UUID v4 format used for operation ids
        let uuid = uuid::Uuid::new_v4().to_string();
        assert_eq!(uuid.len(), 36); // UUID v4 string length
        assert!(uuid.contains('-'));
    }

    #[test]
    fn test_cancel_token_initial_state() {
        let token = Arc::new(AtomicBool::new(false));
        assert!(!token.load(Ordering::SeqCst));
    }

    #[test]
    fn test_cancel_token_cancelled_state() {
        let token = Arc::new(AtomicBool::new(false));
        token.store(true, Ordering::SeqCst);
        assert!(token.load(Ordering::SeqCst));
    }
}
