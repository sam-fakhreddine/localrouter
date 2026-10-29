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

use lr_config::{
    AppConfig, ConfigManager, ContextManagementConfig, ContextManagementOverrides,
    GatewayIndexingPermissions, IndexingState, PermissionState, SkillsPermissions,
};
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

/// Write <root>/<name>/SKILL.md with the given description and body.
fn write_skill(root: &Path, name: &str, description: &str, body: &str) {
    let skill = root.join(name);
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: \"{description}\"\n---\n{body}\n"),
    )
    .unwrap();
}

/// <root>/quokka-guide/{SKILL.md, references/note.md} plus an unrelated
/// <root>/emu-gardening/SKILL.md; returns <root>.
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
    write_skill(
        &root,
        "emu-gardening",
        "Pruning shrubs and watering ferns",
        "# Emu Gardening\n\nWater the ferns at dawn.",
    );
    root
}

/// The context_management block of infra/mcp-gateway/settings.yaml: all
/// indexing switched off.
fn production_context_management() -> ContextManagementConfig {
    ContextManagementConfig {
        gateway_indexing: GatewayIndexingPermissions {
            global: IndexingState::Disable,
            ..Default::default()
        },
        virtual_indexing: GatewayIndexingPermissions {
            global: IndexingState::Disable,
            ..Default::default()
        },
        client_tools_indexing_default: IndexingState::Disable,
        ..Default::default()
    }
}

/// With all indexing off, context management is off for a client unless the
/// client turns it on (an override), which is what "context management on"
/// means under the production config.
fn cm_on() -> Option<ContextManagementOverrides> {
    Some(ContextManagementOverrides {
        context_management_enabled: Some(true),
        catalog_compression_enabled: None,
    })
}

fn build_gateway(skills_root: &Path) -> Arc<McpGateway> {
    build_gateway_with(skills_root, ContextManagementConfig::default())
}

fn build_gateway_with(
    skills_root: &Path,
    context_management: ContextManagementConfig,
) -> Arc<McpGateway> {
    let mut config = AppConfig::default();
    config.skills.paths = vec![skills_root.to_string_lossy().to_string()];
    config.context_management = context_management;

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
    cm: &Option<ContextManagementOverrides>,
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
            cm.clone(),
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

async fn init_session(
    gateway: &McpGateway,
    client_id: &str,
    perms: &SkillsPermissions,
    cm: &Option<ContextManagementOverrides>,
) {
    let init = JsonRpcRequest::with_id(
        0,
        "initialize".to_string(),
        Some(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "1.0"}
        })),
    );
    send(gateway, client_id, perms, cm, init).await;
}

async fn tool_names(
    gateway: &McpGateway,
    client_id: &str,
    perms: &SkillsPermissions,
    cm: &Option<ContextManagementOverrides>,
) -> Vec<String> {
    let req = JsonRpcRequest::with_id(1, "tools/list".to_string(), Some(json!({})));
    let result = send(gateway, client_id, perms, cm, req).await;
    result["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect()
}

async fn call_tool(
    gateway: &McpGateway,
    client_id: &str,
    perms: &SkillsPermissions,
    cm: &Option<ContextManagementOverrides>,
    name: &str,
    arguments: Value,
) -> String {
    let req = JsonRpcRequest::with_id(
        2,
        "tools/call".to_string(),
        Some(json!({"name": name, "arguments": arguments})),
    );
    let result = send(gateway, client_id, perms, cm, req).await;
    result["content"][0]["text"]
        .as_str()
        .expect("text content")
        .to_string()
}

async fn skill_read(
    gateway: &McpGateway,
    client_id: &str,
    perms: &SkillsPermissions,
    cm: &Option<ContextManagementOverrides>,
    arguments: Value,
) -> String {
    call_tool(gateway, client_id, perms, cm, "SkillRead", arguments).await
}

/// IndexSearch over the skills catalog, polling for at most 5 s until the
/// reply contains `wait_for` (indexing may lag the first request).
async fn search_skills(
    gateway: &McpGateway,
    client_id: &str,
    perms: &SkillsPermissions,
    cm: &Option<ContextManagementOverrides>,
    query: &str,
    wait_for: &str,
) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let text = call_tool(
            gateway,
            client_id,
            perms,
            cm,
            "IndexSearch",
            json!({"queries": [query], "source": "catalog:skills"}),
        )
        .await;
        if text.contains(wait_for) || std::time::Instant::now() >= deadline {
            return text;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn lists_skill_read_for_client_with_skills_permission() {
    // Catches: headless gateway registers only the skills server, so a context-managed client sees SkillRead but no IndexSearch/IndexRead
    let root = write_fixture_skill("list");
    let gateway = build_gateway(&root);
    let perms = allow_all();
    init_session(&gateway, "client-list", &perms, &None).await;

    let names = tool_names(&gateway, "client-list", &perms, &None).await;

    assert_eq!(
        names,
        vec![
            "SkillRead".to_string(),
            "IndexSearch".to_string(),
            "IndexRead".to_string()
        ]
    );
}

#[tokio::test]
async fn skill_read_returns_skill_md_body() {
    // Catches: SkillRead(name) does not return the SKILL.md instructions from the configured skills path
    let root = write_fixture_skill("body");
    let gateway = build_gateway(&root);
    let perms = allow_all();
    init_session(&gateway, "client-body", &perms, &None).await;

    let text = skill_read(&gateway, "client-body", &perms, &None, json!({"name": "quokka-guide"})).await;

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
    init_session(&gateway, "client-file", &perms, &None).await;

    let text = skill_read(
        &gateway,
        "client-file",
        &perms,
        &None,
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
    init_session(&gateway, "client-denied", &perms, &None).await;

    let names = tool_names(&gateway, "client-denied", &perms, &None).await;

    assert!(
        !names.iter().any(|n| n == "SkillRead"),
        "SkillRead leaked to client without permission: {names:?}"
    );
}

#[tokio::test]
async fn index_search_finds_skill_by_description() {
    // Catches: IndexSearch over catalog:skills misses a skill whose description matches, or returns an unrelated skill
    let root = write_fixture_skill("search");
    let gateway = build_gateway(&root);
    let perms = allow_all();
    init_session(&gateway, "client-search", &perms, &None).await;
    tool_names(&gateway, "client-search", &perms, &None).await;

    let text = search_skills(&gateway, "client-search", &perms, &None, "procedure", "quokka-guide").await;

    assert!(text.contains("quokka-guide"), "quokka-guide missing from: {text}");
    assert!(
        !text.contains("emu-gardening"),
        "unrelated skill returned: {text}"
    );
}

#[tokio::test]
async fn index_search_finds_skill_under_production_context_config() {
    // Catches: production context_management (all indexing disabled) leaves the skills catalog unsearchable
    let root = write_fixture_skill("search-prod");
    let gateway = build_gateway_with(&root, production_context_management());
    let perms = allow_all();
    let cm = cm_on();
    init_session(&gateway, "client-search-prod", &perms, &cm).await;
    tool_names(&gateway, "client-search-prod", &perms, &cm).await;

    let text = search_skills(&gateway, "client-search-prod", &perms, &cm, "procedure", "quokka-guide").await;

    assert!(text.contains("quokka-guide"), "quokka-guide missing from: {text}");
    assert!(
        !text.contains("emu-gardening"),
        "unrelated skill returned: {text}"
    );
}

#[tokio::test]
async fn skill_read_long_body_is_not_compressed_under_production_config() {
    // Catches: context management compresses a long SkillRead reply into a preview plus an IndexRead pointer
    let root = write_fixture_skill("long");
    let mut body = String::from("# Long Scroll\n\n");
    for i in 0..120 {
        body.push_str(&format!("Step {i}: unroll the scroll a little further.\n"));
    }
    body.push_str("FINAL-LINE-ZEBRA-CROSSING");
    assert!(body.len() > 2000);
    write_skill(&root, "long-scroll", "A very long scroll of steps", &body);
    let gateway = build_gateway_with(&root, production_context_management());
    let perms = allow_all();
    let cm = cm_on();
    init_session(&gateway, "client-long", &perms, &cm).await;
    tool_names(&gateway, "client-long", &perms, &cm).await;

    let text = skill_read(&gateway, "client-long", &perms, &cm, json!({"name": "long-scroll"})).await;

    assert!(
        text.contains("FINAL-LINE-ZEBRA-CROSSING"),
        "last body line missing from: {text}"
    );
    assert!(
        !text.contains("IndexRead"),
        "reply carries an IndexRead pointer: {text}"
    );
}
