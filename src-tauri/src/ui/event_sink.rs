//! Bridges the desktop app's `AppHandle` to the UI-agnostic `EventSink` the
//! library crates take, so they can build without Tauri.

use lr_types::EventSink;
use std::sync::Arc;
use tauri::Emitter;

/// Wrap an app handle in an event sink that emits to the frontend.
pub fn from_app(handle: &tauri::AppHandle) -> EventSink {
    let handle = handle.clone();
    Arc::new(move |event, payload| handle.emit(event, payload).map_err(|e| e.to_string()))
}
