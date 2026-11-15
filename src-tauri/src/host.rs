use crate::error::{AxisError, Result};
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Runtime, Wry};
use tauri_plugin_opener::OpenerExt;

/// What services need from their environment: emit events and open URLs.
/// Implemented by `TauriHost` for the desktop app; keeps services Tauri-free.
pub trait Host: Send + Sync + 'static {
    fn emit_json(&self, event: &'static str, payload: serde_json::Value);
    fn open_url(&self, url: &str) -> Result<()>;
}

pub type HostHandle = Arc<dyn Host>;

/// An event with a Tauri-free name, equal to its `tauri_specta::Event::NAME`.
pub trait AxisEvent: Serialize {
    const NAME: &'static str;
}

pub fn emit<E: AxisEvent>(host: &dyn Host, event: &E) {
    match serde_json::to_value(event) {
        Ok(payload) => host.emit_json(E::NAME, payload),
        Err(e) => log::error!("Failed to serialize event {}: {e}", E::NAME),
    }
}

// Generic over the runtime so tests can wrap a `MockRuntime` handle.
pub struct TauriHost<R: Runtime = Wry>(pub AppHandle<R>);

impl<R: Runtime> Host for TauriHost<R> {
    fn emit_json(&self, event: &'static str, payload: serde_json::Value) {
        if let Err(e) = self.0.emit(event, payload) {
            log::error!("Failed to emit event {event}: {e}");
        }
    }

    fn open_url(&self, url: &str) -> Result<()> {
        self.0
            .opener()
            .open_url(url, None::<&str>)
            .map_err(|e| AxisError::Other(e.to_string()))
    }
}

/// Test host that records emitted events and opened URLs.
#[cfg(any(test, feature = "integration"))]
#[derive(Default)]
pub struct RecordingHost {
    pub events: parking_lot::Mutex<Vec<(String, serde_json::Value)>>,
    pub opened_urls: parking_lot::Mutex<Vec<String>>,
}

#[cfg(any(test, feature = "integration"))]
impl RecordingHost {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn names(&self) -> Vec<String> {
        self.events
            .lock()
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }
}

#[cfg(any(test, feature = "integration"))]
impl Host for RecordingHost {
    fn emit_json(&self, event: &'static str, payload: serde_json::Value) {
        self.events.lock().push((event.to_string(), payload));
    }

    fn open_url(&self, url: &str) -> Result<()> {
        self.opened_urls.lock().push(url.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(desktop)]
    use crate::events::HeadChangedEvent;
    use crate::events::RefChangedEvent;
    #[cfg(desktop)]
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[cfg(desktop)]
    use tauri::Listener;

    #[test]
    fn emit_records_name_and_payload() {
        let host = RecordingHost::new();
        emit(
            host.as_ref(),
            &RefChangedEvent {
                ref_name: "refs/heads/main".into(),
            },
        );
        let events = host.events.lock();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "ref-changed-event");
        assert_eq!(
            events[0].1,
            serde_json::json!({ "refName": "refs/heads/main" })
        );
    }

    // Desktop only: on Android `mock_app()` panics in MockRuntime's `todo!()` `run_on_android_context`.
    #[cfg(desktop)]
    #[test]
    fn tauri_host_emits_to_listeners() {
        let app = tauri::test::mock_app();
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&calls);
        app.handle().listen("head-changed-event", move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
        });

        // Rust listeners are invoked synchronously inside emit.
        emit(&TauriHost(app.handle().clone()), &HeadChangedEvent);

        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
