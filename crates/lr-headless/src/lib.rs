//! LocalRouter without the desktop app.
//!
//! The desktop build answers firewall "Ask" decisions in a Tauri popup. A
//! headless server has no window, so [`approvals`] serves the same decisions
//! as a small web API and page, reachable only by the owner.

pub mod approvals;
