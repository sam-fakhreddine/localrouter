//! Headless LocalRouter server.
//!
//! Ported from `run_gui_mode()` in `src-tauri/src/main.rs`, minus everything
//! that needs Tauri (windows, tray, updater, autostart, dialogs, notifications)
//! and the src-tauri-only modules (launcher, ui, inspection and reverse
//! proxies, local model library and embedded engines). No event sink is set.
//! Firewall "Ask" decisions are answered through the web approvals API.
//!
//! Environment: `LR_ALLOWED_HOSTS` is a comma-separated list of extra host
//! names or IPs (no ports, no wildcards, case-insensitive) accepted in the
//! Host header besides localhost, 127.0.0.1 and [::1]. Example:
//! `LR_ALLOWED_HOSTS=192.168.0.212,mcp-gateway`. Unset or empty keeps the
//! localhost-only DNS rebinding check.

use std::sync::Arc;

use anyhow::Context;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use lr_providers::factory::{
    AnthropicProviderFactory, CerebrasProviderFactory, CloudflareAIProviderFactory,
    CohereProviderFactory, DeepInfraProviderFactory, DigitalOceanProviderFactory,
    GPT4AllProviderFactory, GeminiProviderFactory, GitHubCopilotProviderFactory,
    GitHubModelsProviderFactory, GroqProviderFactory, HuggingFaceProviderFactory,
    JanProviderFactory, KlusterAIProviderFactory, LMStudioProviderFactory, LlamaCppProviderFactory,
    Llm7ProviderFactory, LlmGatewayProviderFactory, LocalAIProviderFactory, MistralProviderFactory,
    NvidiaNimProviderFactory, OllamaProviderFactory, OpenAICodexProviderFactory,
    OpenAICompatibleProviderFactory, OpenAIProviderFactory, OpenCodeGoProviderFactory,
    OpenCodeZenProviderFactory, OpenRouterProviderFactory, PerplexityProviderFactory,
    SystemOneProviderFactory, TogetherAIProviderFactory, VercelAiGatewayProviderFactory,
    XAIProviderFactory, ZhipuProviderFactory,
};
use lr_providers::registry::ProviderRegistry;
use lr_server::ServerManager;

const DEFAULT_APPROVALS_ADDR: &str = "127.0.0.1:3626";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("lr_headless=info,lr_server=info,lr_mcp=info")
            }),
        )
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();

    // Fail before touching anything else when the owner is not configured.
    let owner_login = std::env::var("LR_OWNER_LOGIN").unwrap_or_default();
    let owner_login = owner_login.trim().to_string();
    if owner_login.is_empty() {
        anyhow::bail!(
            "LR_OWNER_LOGIN is not set or empty: it names the only user allowed to answer firewall approvals"
        );
    }
    let approvals_addr = std::env::var("LR_APPROVALS_ADDR")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_APPROVALS_ADDR.to_string());

    info!("Starting LocalRouter (headless)...");
    let config_dir = lr_utils::paths::config_dir().context("cannot resolve config directory")?;
    info!("Configuration directory: {}", config_dir.display());

    // Initialize managers. Same fallback as the desktop app: a config that
    // fails to load is replaced by defaults.
    let mut config_manager = match lr_config::ConfigManager::load().await {
        Ok(cm) => cm,
        Err(e) => {
            warn!("Failed to load config, using defaults: {}", e);
            lr_config::ConfigManager::new(
                lr_config::AppConfig::default(),
                lr_utils::paths::config_file().context("cannot resolve config file path")?,
            )
        }
    };

    // Unified client manager (secrets in the keychain, metadata in config)
    let client_manager = {
        let config = config_manager.get();
        Arc::new(lr_clients::ClientManager::new(config.clients.clone()))
    };

    // Keep ClientManager in sync with config changes
    {
        let client_manager_for_sync = client_manager.clone();
        config_manager.set_client_sync_callback(Arc::new(move |clients| {
            client_manager_for_sync.sync_clients(clients);
        }));
    }

    // Short-lived OAuth access tokens, in memory only
    let token_store = Arc::new(lr_clients::TokenStore::new());

    // Keychain for secure storage
    let keychain = match lr_api_keys::keychain_trait::CachedKeychain::auto() {
        Ok(k) => k,
        Err(e) => {
            error!("Failed to initialize keychain: {}", e);
            lr_api_keys::keychain_trait::CachedKeychain::system()
        }
    };
    match std::env::var("LOCALROUTER_KEYCHAIN").as_deref() {
        Ok("file") => info!("Keychain backend: file (LOCALROUTER_KEYCHAIN=file)"),
        Ok("system") => info!("Keychain backend: system keyring (LOCALROUTER_KEYCHAIN=system)"),
        _ if cfg!(debug_assertions) => {
            info!("Keychain backend: file (debug build default, LOCALROUTER_KEYCHAIN unset)")
        }
        _ => info!("Keychain backend: system keyring (release default, LOCALROUTER_KEYCHAIN unset)"),
    }

    // MCP server manager
    let mcp_server_manager = {
        let config = config_manager.get();
        let manager = Arc::new(lr_mcp::McpServerManager::new());
        manager.load_configs(config.mcp_servers.clone());
        manager
    };

    // Provider registry
    info!("Initializing provider registry...");
    let provider_registry = Arc::new(ProviderRegistry::new());

    info!("Registering provider factories...");
    provider_registry.register_factory(Arc::new(OllamaProviderFactory));
    provider_registry.register_factory(Arc::new(OpenAIProviderFactory));
    provider_registry.register_factory(Arc::new(OpenAICompatibleProviderFactory));
    provider_registry.register_factory(Arc::new(AnthropicProviderFactory));
    provider_registry.register_factory(Arc::new(GeminiProviderFactory));
    provider_registry.register_factory(Arc::new(OpenRouterProviderFactory));
    provider_registry.register_factory(Arc::new(GroqProviderFactory));
    provider_registry.register_factory(Arc::new(MistralProviderFactory));
    provider_registry.register_factory(Arc::new(CohereProviderFactory));
    provider_registry.register_factory(Arc::new(TogetherAIProviderFactory));
    provider_registry.register_factory(Arc::new(PerplexityProviderFactory));
    provider_registry.register_factory(Arc::new(DeepInfraProviderFactory));
    provider_registry.register_factory(Arc::new(CerebrasProviderFactory));
    provider_registry.register_factory(Arc::new(XAIProviderFactory));
    provider_registry.register_factory(Arc::new(LMStudioProviderFactory));
    provider_registry.register_factory(Arc::new(JanProviderFactory));
    provider_registry.register_factory(Arc::new(GPT4AllProviderFactory));
    provider_registry.register_factory(Arc::new(LocalAIProviderFactory));
    provider_registry.register_factory(Arc::new(LlamaCppProviderFactory));
    // Free-tier providers
    provider_registry.register_factory(Arc::new(GitHubModelsProviderFactory));
    provider_registry.register_factory(Arc::new(NvidiaNimProviderFactory));
    provider_registry.register_factory(Arc::new(CloudflareAIProviderFactory));
    provider_registry.register_factory(Arc::new(Llm7ProviderFactory));
    provider_registry.register_factory(Arc::new(KlusterAIProviderFactory));
    provider_registry.register_factory(Arc::new(HuggingFaceProviderFactory));
    provider_registry.register_factory(Arc::new(ZhipuProviderFactory));
    provider_registry.register_factory(Arc::new(DigitalOceanProviderFactory));
    provider_registry.register_factory(Arc::new(OpenCodeZenProviderFactory));
    provider_registry.register_factory(Arc::new(OpenCodeGoProviderFactory));
    provider_registry.register_factory(Arc::new(LlmGatewayProviderFactory));
    provider_registry.register_factory(Arc::new(VercelAiGatewayProviderFactory));
    // System One decision providers for servers the user runs or hosts
    for factory in SystemOneProviderFactory::all() {
        provider_registry.register_factory(Arc::new(factory));
    }
    // Local Embedded providers (engine supervisor, local model library) are not
    // registered: they launch engines and download weights, which a headless
    // server does not do.
    // Subscription providers (OAuth-based)
    provider_registry.register_factory(Arc::new(GitHubCopilotProviderFactory));
    provider_registry.register_factory(Arc::new(OpenAICodexProviderFactory));
    info!(
        "Registered {} provider factories",
        provider_registry.list_provider_types().len()
    );

    // First startup: discover local LLM providers (Ollama, LM Studio)
    {
        let config = config_manager.get();
        if !config.setup_wizard_shown && config.providers.is_empty() {
            info!("First startup detected, discovering local LLM providers...");
            let discovered = lr_providers::factory::discover_local_providers().await;

            if !discovered.is_empty() {
                info!("Discovered {} local provider(s)", discovered.len());
                if let Err(e) = config_manager.update(|cfg| {
                    for provider in &discovered {
                        let provider_config = match provider.provider_type.as_str() {
                            "ollama" => lr_config::ProviderConfig::default_ollama(),
                            "lmstudio" => lr_config::ProviderConfig::default_lmstudio(),
                            "jan" => lr_config::ProviderConfig::default_jan(),
                            "gpt4all" => lr_config::ProviderConfig::default_gpt4all(),
                            _ => continue,
                        };
                        info!(
                            "Auto-configuring discovered provider: {}",
                            provider.instance_name
                        );
                        cfg.providers.push(provider_config);
                    }
                }) {
                    warn!("Failed to save discovered providers to config: {}", e);
                }
            } else {
                info!("No local LLM providers discovered");
            }
        }
    }

    // Load provider instances from configuration
    info!("Loading provider instances from configuration...");
    let providers = config_manager.get().providers;
    for provider_config in providers {
        let provider_type = match provider_config.provider_type {
            lr_config::ProviderType::Ollama => "ollama",
            lr_config::ProviderType::LMStudio => "lmstudio",
            lr_config::ProviderType::OpenAI => "openai",
            lr_config::ProviderType::Anthropic => "anthropic",
            lr_config::ProviderType::Gemini => "gemini",
            lr_config::ProviderType::OpenRouter => "openrouter",
            lr_config::ProviderType::Groq => "groq",
            lr_config::ProviderType::Mistral => "mistral",
            lr_config::ProviderType::Cohere => "cohere",
            lr_config::ProviderType::TogetherAI => "togetherai",
            lr_config::ProviderType::Perplexity => "perplexity",
            lr_config::ProviderType::DeepInfra => "deepinfra",
            lr_config::ProviderType::Cerebras => "cerebras",
            lr_config::ProviderType::XAI => "xai",
            lr_config::ProviderType::Jan => "jan",
            lr_config::ProviderType::GPT4All => "gpt4all",
            lr_config::ProviderType::LocalAI => "localai",
            lr_config::ProviderType::LlamaCpp => "llamacpp",
            lr_config::ProviderType::GitHubModels => "github_models",
            lr_config::ProviderType::NvidiaNim => "nvidia_nim",
            lr_config::ProviderType::CloudflareAI => "cloudflare_ai",
            lr_config::ProviderType::Llm7 => "llm7",
            lr_config::ProviderType::KlusterAI => "kluster_ai",
            lr_config::ProviderType::HuggingFace => "huggingface",
            lr_config::ProviderType::Zhipu => "zhipu",
            lr_config::ProviderType::DigitalOcean => "digitalocean",
            lr_config::ProviderType::OpenCodeZen => "opencode_zen",
            lr_config::ProviderType::OpenCodeGo => "opencode_go",
            lr_config::ProviderType::ChatGPTPlus => "openai-chatgpt-plus",
            lr_config::ProviderType::TypeSafe => "typesafe",
            lr_config::ProviderType::Laya => "laya",
            lr_config::ProviderType::Kev => "kev",
            lr_config::ProviderType::SystemOneCompatible => "systemone_compatible",
            lr_config::ProviderType::Von => "von",
            lr_config::ProviderType::Decider => "decider",
            lr_config::ProviderType::Ollaya => "ollaya",
            lr_config::ProviderType::LlamaCppEmbedded => "llamacpp_embedded",
            lr_config::ProviderType::SdCppEmbedded => "sdcpp_embedded",
            lr_config::ProviderType::LlmGateway => "llmgateway",
            lr_config::ProviderType::VercelAiGateway => "vercel_ai_gateway",
            lr_config::ProviderType::Custom => "openai_compatible",
        };

        // Convert provider_config JSON to HashMap
        let mut config_map = std::collections::HashMap::new();
        if let Some(provider_cfg) = provider_config.provider_config {
            if let Some(obj) = provider_cfg.as_object() {
                for (key, value) in obj {
                    if let Some(value_str) = value.as_str() {
                        config_map.insert(key.clone(), value_str.to_string());
                    } else {
                        config_map.insert(key.clone(), value.to_string());
                    }
                }
            }
        }

        // Inject api_key from keychain if not already in config_map
        if !config_map.contains_key("api_key") {
            match lr_providers::key_storage::get_provider_key(&provider_config.name) {
                Ok(Some(api_key)) => {
                    config_map.insert("api_key".to_string(), api_key);
                }
                Ok(None) => {}
                Err(e) => {
                    warn!(
                        "Failed to retrieve API key for provider '{}' from keychain: {}",
                        provider_config.name, e
                    );
                }
            }
        }

        // Inject custom headers from keychain (they may carry credentials)
        if !config_map.contains_key("custom_headers") {
            let account = lr_providers::key_storage::custom_headers_account(&provider_config.name);
            match lr_providers::key_storage::get_provider_key(&account) {
                Ok(Some(headers)) => {
                    config_map.insert("custom_headers".to_string(), headers);
                }
                Ok(None) => {}
                Err(e) => {
                    warn!(
                        "Failed to retrieve custom headers for provider '{}' from keychain: {}",
                        provider_config.name, e
                    );
                }
            }
        }

        if let Err(e) = provider_registry
            .create_provider(
                provider_config.name.clone(),
                provider_type.to_string(),
                config_map,
            )
            .await
        {
            warn!("Failed to load provider '{}': {}", provider_config.name, e);
            continue;
        }

        if let Err(e) =
            provider_registry.set_provider_enabled(&provider_config.name, provider_config.enabled)
        {
            warn!(
                "Failed to set provider '{}' enabled state: {}",
                provider_config.name, e
            );
        }
    }
    info!(
        "Loaded {} provider instances",
        config_manager.get().providers.len()
    );

    // OAuth manager for subscription-based providers
    info!("Initializing OAuth manager...");
    let oauth_storage_path = config_dir.join("oauth_credentials.json");
    let oauth_storage = Arc::new(
        lr_providers::oauth::storage::OAuthStorage::new(oauth_storage_path)
            .await
            .context("failed to initialize OAuth storage")?,
    );
    let oauth_manager = Arc::new(lr_providers::oauth::OAuthManager::new(oauth_storage));

    info!("Registering OAuth providers...");
    oauth_manager.register_provider(Arc::new(
        lr_providers::oauth::github_copilot::GitHubCopilotOAuthProvider::new(),
    ));
    oauth_manager.register_provider(Arc::new(
        lr_providers::oauth::openai_codex::OpenAICodexOAuthProvider::new(keychain.clone()),
    ));
    oauth_manager.register_provider(Arc::new(
        lr_providers::oauth::anthropic_claude::AnthropicClaudeOAuthProvider::new(keychain.clone()),
    ));
    info!("Registered 3 OAuth providers");

    // Rate limiter
    info!("Initializing rate limiter...");
    let rate_limiter = Arc::new(lr_router::RateLimiterManager::new(None));

    // Metrics collector
    info!("Initializing metrics collector...");
    let metrics_db_path = config_dir.join("metrics.db");
    let metrics_db = Arc::new(
        lr_monitoring::storage::MetricsDatabase::new(metrics_db_path)
            .context("failed to initialize metrics database")?,
    );
    let metrics_collector = Arc::new(lr_monitoring::metrics::MetricsCollector::new(metrics_db));

    // RouteLLM is left off: the model weights are downloaded on demand and a
    // headless server must never fetch them at startup.
    info!("RouteLLM disabled in headless mode");

    // Free tier manager
    info!("Initializing free tier manager...");
    let free_tier_manager = Arc::new(lr_router::FreeTierManager::load(
        &config_dir.join("free_tier_state.json"),
    ));

    // Shared health cache (used by both router and server)
    let health_cache = Arc::new(lr_providers::health_cache::HealthCacheManager::new());

    // Router
    info!("Initializing router...");
    let config_manager_arc = Arc::new(config_manager.clone());
    let app_router = Arc::new(
        lr_router::Router::new(
            config_manager_arc.clone(),
            provider_registry.clone(),
            rate_limiter.clone(),
            metrics_collector.clone(),
            free_tier_manager.clone(),
        )
        .with_health_cache(health_cache.clone())
        .with_routellm(None),
    );

    // Server
    info!("Initializing web server...");
    let server_manager = Arc::new(ServerManager::new());

    let server_config = {
        let config = config_manager.get();
        lr_server::ServerConfig {
            host: config.server.host.clone(),
            port: config.server.port,
            enable_cors: config.server.enable_cors,
        }
    };

    server_manager
        .start(
            server_config,
            lr_server::manager::ServerDependencies {
                router: app_router.clone(),
                mcp_server_manager: mcp_server_manager.clone(),
                rate_limiter: rate_limiter.clone(),
                provider_registry: provider_registry.clone(),
                config_manager: config_manager_arc.clone(),
                client_manager: client_manager.clone(),
                token_store: token_store.clone(),
                metrics_collector: metrics_collector.clone(),
                health_cache: Some(health_cache.clone()),
            },
        )
        .await
        .context("failed to start the LocalRouter server")?;

    // Approvals web API on the gateway's firewall manager
    let gateway = server_manager
        .get_mcp_gateway()
        .context("MCP gateway is not available after server start")?;
    let app = lr_headless::approvals::router(gateway.firewall_manager.clone(), owner_login);
    let listener = tokio::net::TcpListener::bind(&approvals_addr)
        .await
        .with_context(|| format!("cannot bind approvals address {approvals_addr}"))?;
    info!("Approvals API listening on {}", approvals_addr);

    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let approvals_task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stop_rx.await;
            })
            .await
    });

    wait_for_shutdown_signal().await;
    info!("Shutdown signal received");

    let _ = stop_tx.send(());
    match approvals_task.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => warn!("Approvals server error: {}", e),
        Err(e) => warn!("Approvals server task failed: {}", e),
    }
    server_manager.stop().await;
    info!("Stopped");
    Ok(())
}

/// Resolves on SIGTERM or Ctrl-C.
async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(e) => {
                warn!("Cannot install SIGTERM handler: {}", e);
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
