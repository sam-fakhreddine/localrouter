//! RED tests: the headless gateway serves AgentSkills through SkillRead.
//!
//! Expected new public API in the lr-headless lib (port of the Tauri wiring in
//! src-tauri/src/main.rs: SkillManager setup plus registering
//! `SkillsVirtualServer` and `ContextModeVirtualServer` on the gateway):
//!
//! ```ignore
//! // crates/lr-headless/src/skills.rs, exported as `pub mod skills;`
//! pub fn register(
//!     gateway: &lr_mcp::gateway::McpGateway,
//!     config: &lr_config::AppConfig,
//! ) -> std::sync::Arc<lr_skills::SkillManager>;
//! ```
//!
//! It scans `config.skills.paths` (minus `disabled_skills`), starts the skill
//! file watcher, registers the skills virtual server (built from
//! `config.skills` and `config.context_management`) and the context-mode
//! virtual server on `gateway`, and returns the manager so the caller can keep
//! it alive. Call it before the gateway is put in an `Arc`.

use lr_config::{AppConfig, ConfigManager, PermissionState, SkillsPermissions};
use lr_mcp::gateway::{GatewayConfig, McpGateway};
use lr_mcp::protocol::JsonRpcRequest;
use lr_mcp::McpServerManager;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SKILL_BODY_LINE: &str = "Run the quokka procedure exactly three times.";
const NOTE_TEXT: &str = "# Quokka Note\n\nThe quokka procedure ends with a handshake.\n";

fn scratch_dir(label: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("headless-skills-{label}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// <root>/quokka-guide/{SKILL.md, references/note.md}; returns <root>.
fn write_fixture_skill(label: &str) -> PathBuf {
    let root = scratch_dir(label);
    let skill = root.join("quokka-guide");
    std::fs::create_dir_all(skill.join("references")).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        format!(
            "---\nname: quokka-guide\ndescription: \"How to run the quokka procedure\"\n---\n# Quokka Guide\n\n{SKILL_BODY_LINE}\n"
        ),
    )
    .unwrap();
    std::fs::write(skill.join("references").join("note.md"), NOTE_TEXT).unwrap();
    root
}

fn build_gateway(skills_root: &Path) -> Arc<McpGateway> {
    let mut config = AppConfig::default();
    config.skills.paths = vec![skills_root.to_string_lossy().to_string()];

    let work = scratch_dir("router");
    let config_manager = Arc::new(ConfigManager::new(
        config.clone(),
        work.join("settings.yaml"),
    ));
    let metrics_db = Arc::new(
        lr_monitoring::storage::MetricsDatabase::new(work.join("metrics.db")).unwrap(),
    );
    let router = Arc::new(lr_router::Router::new(
        config_manager,
        Arc::new(lr_providers::registry::ProviderRegistry::new()),
        Arc::new(lr_router::RateLimiterManager::new(None)),
        Arc::new(lr_monitoring::metrics::MetricsCollector::new(metrics_db)),
        Arc::new(lr_router::FreeTierManager::new(None)),
    ));

    let gateway = McpGateway::new(
        Arc::new(McpServerManager::new()),
        GatewayConfig::default(),
        router,
    );
    let _skill_manager = lr_headless::skills::register(&gateway, &config);
    Arc::new(gateway)
}

fn allow_all() -> SkillsPermissions {
    SkillsPermissions {
        global: PermissionState::Allow,
        ..Default::default()
    }
}

async fn send(
    gateway: &McpGateway,
    client_id: &str,
    perms: &SkillsPermissions,
    req: JsonRpcRequest,
) -> Value {
    let resp = gateway
        .handle_request_with_skills(
            client_id,
            None,
            vec![],
            vec![],
            lr_config::McpPermissions::default(),
            perms.clone(),
            "Test Client".to_string(),
            PermissionState::Off,
            PermissionState::Off,
            None,
            None,
            PermissionState::default(),
            PermissionState::default(),
            None,
            lr_config::ClientMode::default(),
            req,
            None,
        )
        .await
        .expect("gateway request should succeed");
    resp.result.expect("response should have a result")
}

async fn init_session(gateway: &McpGateway, client_id: &str, perms: &SkillsPermissions) {
    let init = JsonRpcRequest::with_id(
        0,
        "initialize".to_string(),
        Some(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "1.0"}
        })),
    );
    send(gateway, client_id, perms, init).await;
}

async fn tool_names(gateway: &McpGateway, client_id: &str, perms: &SkillsPermissions) -> Vec<String> {
    let req = JsonRpcRequest::with_id(1, "tools/list".to_string(), Some(json!({})));
    let result = send(gateway, client_id, perms, req).await;
    result["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect()
}

async fn skill_read(
    gateway: &McpGateway,
    client_id: &str,
    perms: &SkillsPermissions,
    arguments: Value,
) -> String {
    let req = JsonRpcRequest::with_id(
        2,
        "tools/call".to_string(),
        Some(json!({"name": "SkillRead", "arguments": arguments})),
    );
    let result = send(gateway, client_id, perms, req).await;
    result["content"][0]["text"]
        .as_str()
        .expect("text content")
        .to_string()
}

#[tokio::test]
async fn lists_skill_read_for_client_with_skills_permission() {
    // Catches: headless gateway never registers the skills virtual server, so tools/list returns no SkillRead
    let root = write_fixture_skill("list");
    let gateway = build_gateway(&root);
    let perms = allow_all();
    init_session(&gateway, "client-list", &perms).await;

    let names = tool_names(&gateway, "client-list", &perms).await;

    assert_eq!(names, vec!["SkillRead".to_string()]);
}

#[tokio::test]
async fn skill_read_returns_skill_md_body() {
    // Catches: SkillRead(name) does not return the SKILL.md instructions from the configured skills path
    let root = write_fixture_skill("body");
    let gateway = build_gateway(&root);
    let perms = allow_all();
    init_session(&gateway, "client-body", &perms).await;

    let text = skill_read(&gateway, "client-body", &perms, json!({"name": "quokka-guide"})).await;

    assert!(
        text.contains(SKILL_BODY_LINE),
        "SKILL.md body missing from: {text}"
    );
}

#[tokio::test]
async fn skill_read_with_path_returns_reference_file() {
    // Catches: SkillRead(name, path) fails to return a skill's reference file text
    let root = write_fixture_skill("file");
    let gateway = build_gateway(&root);
    let perms = allow_all();
    init_session(&gateway, "client-file", &perms).await;

    let text = skill_read(
        &gateway,
        "client-file",
        &perms,
        json!({"name": "quokka-guide", "path": "references/note.md"}),
    )
    .await;

    assert_eq!(text, NOTE_TEXT);
}

#[tokio::test]
async fn client_without_skills_permission_sees_no_skills_tool() {
    // Catches: skills tool is exposed to a client that has no skills_permissions
    let root = write_fixture_skill("denied");
    let gateway = build_gateway(&root);
    let perms = SkillsPermissions::default();
    init_session(&gateway, "client-denied", &perms).await;

    let names = tool_names(&gateway, "client-denied", &perms).await;

    assert!(
        !names.iter().any(|n| n == "SkillRead"),
        "SkillRead leaked to client without permission: {names:?}"
    );
}
