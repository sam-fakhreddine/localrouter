#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use super::super::*;
    use crate::protocol::{McpPrompt, McpResource, McpTool};
    use serde_json::json;

    #[test]
    fn test_namespace_parsing() {
        // Valid namespaces
        assert_eq!(
            types::parse_namespace("filesystem__read_file"),
            Some(("filesystem".to_string(), "read_file".to_string()))
        );

        assert_eq!(
            types::parse_namespace("github__create_issue"),
            Some(("github".to_string(), "create_issue".to_string()))
        );

        // Edge cases
        assert_eq!(types::parse_namespace("no_separator"), None);
        assert_eq!(types::parse_namespace("__no_server"), None);
        assert_eq!(types::parse_namespace("no_tool__"), None);
        assert_eq!(types::parse_namespace(""), None);
    }

    #[test]
    fn test_namespace_application() {
        assert_eq!(
            types::apply_namespace("filesystem", "read_file"),
            "filesystem__read_file"
        );

        assert_eq!(
            types::apply_namespace("github", "create_issue"),
            "github__create_issue"
        );
    }

    #[test]
    fn test_namespace_roundtrip() {
        let server = "filesystem";
        let tool = "read_file";
        let namespaced = types::apply_namespace(server, tool);
        let (parsed_server, parsed_tool) = types::parse_namespace(&namespaced).unwrap();

        assert_eq!(parsed_server, server);
        assert_eq!(parsed_tool, tool);
    }

    #[test]
    fn test_merge_tools() {
        let tool1 = McpTool {
            name: "read_file".to_string(),
            description: Some("Read a file".to_string()),
            input_schema: json!({"type": "object"}),
        };

        let tool2 = McpTool {
            name: "create_issue".to_string(),
            description: Some("Create an issue".to_string()),
            input_schema: json!({"type": "object"}),
        };

        let server_tools = vec![
            ("filesystem".to_string(), vec![tool1]),
            ("github".to_string(), vec![tool2]),
        ];

        let merged = merger::merge_tools(server_tools, &[], None);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].name, "filesystem__read_file");
        assert_eq!(merged[0].server_id, "filesystem");
        assert_eq!(merged[0].original_name, "read_file");
        assert_eq!(merged[1].name, "github__create_issue");
        assert_eq!(merged[1].server_id, "github");
        assert_eq!(merged[1].original_name, "create_issue");
    }

    #[test]
    fn test_merge_resources() {
        let resource1 = McpResource {
            name: "config".to_string(),
            uri: "file:///config.json".to_string(),
            description: Some("Config file".to_string()),
            mime_type: Some("application/json".to_string()),
        };

        let resource2 = McpResource {
            name: "repo".to_string(),
            uri: "github://myrepo".to_string(),
            description: Some("Repository".to_string()),
            mime_type: None,
        };

        let server_resources = vec![
            ("filesystem".to_string(), vec![resource1]),
            ("github".to_string(), vec![resource2]),
        ];

        let merged = merger::merge_resources(server_resources, &[], None);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].name, "filesystem__config");
        assert_eq!(merged[0].uri, "file:///config.json");
        assert_eq!(merged[1].name, "github__repo");
        assert_eq!(merged[1].uri, "github://myrepo");
    }

    #[test]
    fn test_merge_prompts() {
        let prompt1 = McpPrompt {
            name: "commit_template".to_string(),
            description: Some("Git commit template".to_string()),
            arguments: None,
        };

        let prompt2 = McpPrompt {
            name: "pr_template".to_string(),
            description: Some("PR template".to_string()),
            arguments: None,
        };

        let server_prompts = vec![
            ("git".to_string(), vec![prompt1]),
            ("github".to_string(), vec![prompt2]),
        ];

        let merged = merger::merge_prompts(server_prompts, &[], None);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].name, "git__commit_template");
        assert_eq!(merged[1].name, "github__pr_template");
    }

    #[test]
    fn test_merge_initialize_results() {
        let result1 = types::InitializeResult {
            protocol_version: "2024-11-05".to_string(),
            capabilities: types::ServerCapabilities {
                tools: Some(types::ToolsCapability {
                    list_changed: Some(true),
                }),
                ..Default::default()
            },
            server_info: types::ServerInfo {
                name: "Filesystem Server".to_string(),
                version: "1.0.0".to_string(),
                description: Some("File operations".to_string()),
            },
            instructions: None,
        };

        let result2 = types::InitializeResult {
            protocol_version: "2024-11-05".to_string(),
            capabilities: types::ServerCapabilities {
                resources: Some(types::ResourcesCapability {
                    list_changed: Some(true),
                    subscribe: Some(true),
                }),
                ..Default::default()
            },
            server_info: types::ServerInfo {
                name: "GitHub Server".to_string(),
                version: "1.0.0".to_string(),
                description: None,
            },
            instructions: None,
        };

        let results = vec![
            ("filesystem".to_string(), result1),
            ("github".to_string(), result2),
        ];

        let merged = merger::merge_initialize_results(results, vec![]);

        assert_eq!(merged.protocol_version, "2024-11-05");
        assert!(merged.capabilities.tools.is_some());
        assert!(merged.capabilities.resources.is_some());
        assert_eq!(merged.server_info.name, "LocalRouter Unified Gateway");
    }

    #[test]
    fn test_should_broadcast() {
        assert!(router::should_broadcast("initialize"));
        assert!(router::should_broadcast("tools/list"));
        assert!(router::should_broadcast("resources/list"));
        assert!(router::should_broadcast("prompts/list"));
        assert!(router::should_broadcast("logging/setLevel"));
        assert!(router::should_broadcast("ping"));

        assert!(!router::should_broadcast("tools/call"));
        assert!(!router::should_broadcast("resources/read"));
        assert!(!router::should_broadcast("prompts/get"));
    }

    #[test]
    fn test_session_creation() {
        use std::time::Duration;

        let session = session::GatewaySession::new(
            "client-123".to_string(),
            vec!["filesystem".to_string(), "github".to_string()],
            Duration::from_secs(3600),
            300,
            Vec::new(),
        );

        assert_eq!(session.client_id, "client-123");
        assert_eq!(session.allowed_servers.len(), 2);
        assert_eq!(session.server_init_status.len(), 2);
        assert!(!session.is_expired());
    }

    #[test]
    fn test_session_expiration() {
        use std::time::Duration;

        let mut session = session::GatewaySession::new(
            "client-123".to_string(),
            vec!["filesystem".to_string()],
            Duration::from_millis(100),
            300,
            Vec::new(),
        );

        assert!(!session.is_expired());

        // Wait for expiration
        std::thread::sleep(Duration::from_millis(150));
        assert!(session.is_expired());

        // Touch should reset expiration
        session.touch();
        assert!(!session.is_expired());
    }

    #[test]
    fn test_cached_list_validity() {
        use std::time::Duration;

        let cached = types::CachedList::new(vec!["item1".to_string()], Duration::from_millis(100));

        assert!(cached.is_valid());

        std::thread::sleep(Duration::from_millis(150));
        assert!(!cached.is_valid());
    }

    #[test]
    fn test_gateway_config_default_includes_mcp_settings() {
        let config = types::GatewayConfig::default();
        assert_eq!(config.sampling, lr_config::SamplingBehavior::Passthrough);
        assert_eq!(
            config.elicitation_mode,
            lr_config::ElicitationMode::Passthrough
        );
    }

    #[test]
    fn test_server_capabilities_with_completions() {
        let capabilities = types::ServerCapabilities {
            completions: Some(types::CompletionsCapability {}),
            ..Default::default()
        };
        assert!(capabilities.completions.is_some());

        // Default should have no completions
        let default_caps = types::ServerCapabilities::default();
        assert!(default_caps.completions.is_none());
    }

    #[test]
    fn test_completions_capability_merging() {
        // Server 1 has tools, no completions
        let result1 = types::InitializeResult {
            protocol_version: "2024-11-05".to_string(),
            capabilities: types::ServerCapabilities {
                tools: Some(types::ToolsCapability {
                    list_changed: Some(true),
                }),
                ..Default::default()
            },
            server_info: types::ServerInfo {
                name: "Server A".to_string(),
                version: "1.0.0".to_string(),
                description: None,
            },
            instructions: None,
        };

        // Server 2 has completions
        let result2 = types::InitializeResult {
            protocol_version: "2024-11-05".to_string(),
            capabilities: types::ServerCapabilities {
                completions: Some(types::CompletionsCapability {}),
                ..Default::default()
            },
            server_info: types::ServerInfo {
                name: "Server B".to_string(),
                version: "1.0.0".to_string(),
                description: None,
            },
            instructions: None,
        };

        let results = vec![
            ("server-a".to_string(), result1),
            ("server-b".to_string(), result2),
        ];

        let merged = merger::merge_initialize_results(results, vec![]);

        // Merged should have both tools and completions
        assert!(merged.capabilities.tools.is_some());
        assert!(merged.capabilities.completions.is_some());
    }

    #[test]
    fn test_resource_templates_list_broadcast() {
        // resources/templates/list should be broadcast
        assert!(router::should_broadcast("resources/templates/list"));

        // Other template-related methods should not
        assert!(!router::should_broadcast("resources/templates/read"));
    }

    #[test]
    fn test_parse_namespace_for_completion() {
        // Completions use namespaced refs like "server__ref/name"
        let result = types::parse_namespace("filesystem__file:///path/to/file");
        assert!(result.is_some());
        let (server, original) = result.unwrap();
        assert_eq!(server, "filesystem");
        assert_eq!(original, "file:///path/to/file");

        // Prompt argument completion ref
        let result = types::parse_namespace("github__pr_template");
        assert!(result.is_some());
        let (server, original) = result.unwrap();
        assert_eq!(server, "github");
        assert_eq!(original, "pr_template");
    }
}

/// Gateway tests against a real stdio MCP upstream (a tiny python server).
#[cfg(test)]
mod lazy_tools_call_tests {
    use crate::gateway::types::GatewayConfig;
    use crate::gateway::McpGateway;
    use crate::manager::McpServerManager;
    use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
    use lr_config::{
        AppConfig, ConfigManager, McpServerConfig, McpTransportConfig, McpTransportType,
    };
    use lr_providers::registry::ProviderRegistry;
    use lr_router::free_tier::FreeTierManager;
    use lr_router::rate_limit::RateLimiterManager;
    use lr_router::Router;
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::sync::Arc;

    const UPSTREAM_PY: &str = r#"
import sys, json
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    method = msg.get("method")
    if "id" not in msg:
        continue
    if method == "initialize":
        res = {"protocolVersion": "2025-03-26", "capabilities": {"tools": {}}, "serverInfo": {"name": "fake", "version": "1"}}
    elif method == "tools/list":
        res = {"tools": [{"name": "web_search", "description": "search", "inputSchema": {"type": "object", "properties": {}}}]}
    elif method == "tools/call":
        res = {"content": [{"type": "text", "text": "upstream-result:" + msg["params"]["name"]}], "isError": False}
    else:
        res = {}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": res}) + "\n")
    sys.stdout.flush()
"#;

    struct Env {
        gateway: McpGateway,
        server_id: String,
    }

    async fn setup() -> Env {
        let dir = std::env::temp_dir().join(format!("lr-lazy-tools-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("upstream.py");
        std::fs::write(&script, UPSTREAM_PY).unwrap();

        let manager = Arc::new(McpServerManager::new_for_test());
        let config = McpServerConfig::new(
            "web".to_string(),
            McpTransportType::Stdio,
            McpTransportConfig::Stdio {
                command: format!("python3 -u {}", script.display()),
                args: vec![],
                env: HashMap::new(),
                cwd: None,
            },
        );
        let server_id = config.id.clone();
        manager.add_config(config);

        let config_manager = Arc::new(ConfigManager::new(
            AppConfig::default(),
            dir.join("config.yaml"),
        ));
        let metrics_db = Arc::new(
            lr_monitoring::storage::MetricsDatabase::new(dir.join("metrics")).unwrap(),
        );
        let metrics = Arc::new(lr_monitoring::metrics::MetricsCollector::new(metrics_db));
        let router = Arc::new(Router::new(
            config_manager,
            Arc::new(ProviderRegistry::new()),
            Arc::new(RateLimiterManager::new(None)),
            metrics,
            Arc::new(FreeTierManager::new(None)),
        ));
        let gateway = McpGateway::new(manager, GatewayConfig::default(), router);
        Env { gateway, server_id }
    }

    async fn send(env: &Env, id: Option<u64>, method: &str, params: Option<Value>) -> JsonRpcResponse {
        let request = JsonRpcRequest::new(id.map(|i| json!(i)), method.to_string(), params);
        env.gateway
            .handle_request("client-lazy", vec![env.server_id.clone()], vec![], request)
            .await
            .expect("gateway request failed")
    }

    async fn init(env: &Env) {
        let r = send(
            env,
            Some(1),
            "initialize",
            Some(json!({"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}})),
        )
        .await;
        assert!(r.error.is_none(), "initialize failed: {:?}", r.error);
        send(env, None, "notifications/initialized", None).await;
    }

    // Catches: tools/call without a prior tools/list returning tool_not_found
    // instead of fetching the upstream tools once and calling the tool.
    #[tokio::test]
    async fn tools_call_without_prior_list_reaches_upstream() {
        let env = setup().await;
        init(&env).await;

        let r = send(
            &env,
            Some(2),
            "tools/call",
            Some(json!({"name": "web__web_search", "arguments": {}})),
        )
        .await;

        assert!(r.error.is_none(), "unexpected error: {:?}", r.error);
        let text = r.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(text, "upstream-result:web_search");
    }

    // Catches: an unknown tool name being accepted, or erroring with something
    // other than tool_not_found, after the lazy fetch.
    #[tokio::test]
    async fn tools_call_unknown_name_after_fetch_is_tool_not_found() {
        let env = setup().await;
        init(&env).await;

        let r = send(
            &env,
            Some(2),
            "tools/call",
            Some(json!({"name": "web__no_such_tool", "arguments": {}})),
        )
        .await;

        let err = r.error.expect("expected an error");
        assert_eq!(err.message, "Tool not found: web__no_such_tool");
    }
}
