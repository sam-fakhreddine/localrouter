//! AgentSkills for the headless server.
//!
//! Port of the skills wiring in the desktop app's `src-tauri/src/main.rs`,
//! without the Tauri parts (event sinks, managed state, marketplace). The
//! context-mode server (IndexSearch/IndexRead) is registered after the skills
//! server, so clients can search the skills catalog. There is no embedding
//! model here, so there is no vector search.

use lr_config::AppConfig;
use lr_mcp::gateway::context_mode::ContextModeVirtualServer;
use lr_mcp::gateway::virtual_skills::SkillsVirtualServer;
use lr_mcp::gateway::McpGateway;
use lr_skills::{SkillManager, SkillWatcher};
use std::sync::Arc;
use tracing::{info, warn};

/// Scan the configured skill paths, watch them for changes, and register the
/// skills and context-mode virtual servers on `gateway`.
///
/// An empty `skills.paths` is fine: no skills are found and nothing fails.
/// The returned manager is also held by the registered skills server; the
/// caller keeps it for the life of the process.
pub fn register(gateway: &McpGateway, config: &AppConfig) -> Arc<SkillManager> {
    let skills_config = config.skills.clone();
    let context_management = config.context_management.clone();

    let skill_manager = SkillManager::new();
    skill_manager.initial_scan(&skills_config.paths, &skills_config.disabled_skills);
    skill_manager.start_cleanup_task();
    let skill_manager = Arc::new(skill_manager);

    // Rescan when a skill file changes. The paths are the ones read at start;
    // headless has no settings UI, so they do not change while it runs.
    let manager_for_watcher = skill_manager.clone();
    let rescan_paths = skills_config.paths.clone();
    let rescan_disabled = skills_config.disabled_skills.clone();
    match SkillWatcher::start(
        skills_config.paths.clone(),
        Arc::new(move |_affected_paths| {
            manager_for_watcher.rescan(&rescan_paths, &rescan_disabled);
        }),
    ) {
        // The watcher stops when dropped. Nothing owns it for the life of the
        // server, so leak it on purpose; it runs until the process exits.
        Ok(watcher) => {
            Box::leak(Box::new(watcher));
            info!("Skills file watcher started");
        }
        Err(e) => warn!("Failed to start skills file watcher: {}", e),
    }

    gateway.register_virtual_server(Arc::new(SkillsVirtualServer::new(
        skill_manager.clone(),
        context_management.clone(),
        skills_config,
    )));
    // Registered after the skills server, as in the desktop app. No embedding
    // service is set (headless has no embedding model), so vector search stays
    // off and IndexSearch uses keyword search only.
    gateway.register_virtual_server(Arc::new(ContextModeVirtualServer::new(
        context_management,
    )));
    info!("Skills system initialized");

    skill_manager
}
