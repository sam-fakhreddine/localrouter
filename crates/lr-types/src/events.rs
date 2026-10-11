//! UI-agnostic event emission.
//!
//! Library crates used to hold an optional `tauri::AppHandle` just to emit
//! events to the desktop frontend. They now hold an [`EventSink`] instead, so
//! they build without Tauri (headless server). The desktop app wraps its
//! `AppHandle` in a sink; a headless build can pass none or its own.

use std::sync::Arc;

/// Receives a named event with a JSON payload. Returns an error string when
/// delivery fails, so callers can keep their existing error logging.
pub type EventSink = Arc<dyn Fn(&str, serde_json::Value) -> Result<(), String> + Send + Sync>;

/// Serialize `payload` and hand it to `sink`.
pub fn emit_event<T: serde::Serialize + ?Sized>(
    sink: &EventSink,
    event: &str,
    payload: &T,
) -> Result<(), String> {
    let value = serde_json::to_value(payload).map_err(|e| e.to_string())?;
    sink(event, value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn emit_event_passes_name_and_serialized_payload() {
        // Catches: a sink receiving the wrong event name or an unserialized payload
        let seen: Arc<Mutex<Vec<(String, serde_json::Value)>>> = Arc::default();
        let s = seen.clone();
        let sink: EventSink = Arc::new(move |e, v| {
            s.lock().unwrap().push((e.to_string(), v));
            Ok(())
        });
        emit_event(&sink, "skills-changed", &serde_json::json!({"n": 2})).unwrap();
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &[("skills-changed".to_string(), serde_json::json!({"n": 2}))]
        );
    }

    #[test]
    fn emit_event_returns_the_sink_error() {
        // Catches: delivery failures being swallowed instead of reaching the caller's log
        let sink: EventSink = Arc::new(|_, _| Err("window closed".to_string()));
        assert_eq!(emit_event(&sink, "x", &()), Err("window closed".to_string()));
    }
}
