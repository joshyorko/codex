use std::path::Path;
use std::sync::Arc;

use codex_extension_api::ContextContributor;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::NoopTurnItemEmitter;
use codex_extension_api::PromptSlot;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolContributor;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolName;
use codex_extension_api::ToolPayload;
use codex_extension_api::TurnInputContext;
use codex_extension_api::TurnInputContributor;
use codex_extension_api::TurnItemContributor;
use codex_protocol::items::AgentMessageContent;
use codex_protocol::items::AgentMessageItem;
use codex_protocol::items::ReasoningItem;
use codex_protocol::items::TurnItem;
use codex_protocol::items::UserMessageItem;
use codex_protocol::user_input::UserInput;
use codex_tools::ToolOutput;
use codex_utils_absolute_path::test_support::PathBufExt;
use codex_utils_absolute_path::test_support::PathExt;
use codex_utils_absolute_path::test_support::test_path_buf;
use codex_utils_output_truncation::TruncationPolicy;
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::extension::MemoriesExtension;
use crate::extension::MemoriesExtensionConfig;
use crate::honcho::HonchoMemoryContext;
use crate::honcho::HonchoMemoryMessage;
use crate::honcho::InMemoryHonchoMemoryClient;
use crate::local::LocalMemoriesBackend;
use crate::portable_schema::LocalCodexMemorySyncRequest;
use crate::portable_schema::LocalCodexMemorySyncResponse;
use crate::portable_schema::PortableMemoryConclusion;
use crate::portable_schema::PortableMemoryContext;
use crate::portable_schema::PortableMemorySettings;
use crate::portable_schema::VisibleMemoryMessage;
use crate::provider::MemoryProvider;
use crate::provider::PortableMemoryError;
use crate::provider::ProviderFuture;
use crate::runtime::PortableMemoryRuntime;
use crate::selected::SelectedMemoriesBackend;

#[test]
fn memory_tool_namespace_matches_responses_api_identifier() {
    assert!(!crate::MEMORY_TOOLS_NAMESPACE.is_empty());
    assert!(
        crate::MEMORY_TOOLS_NAMESPACE
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    );
}

#[test]
fn tools_are_not_contributed_without_thread_config() {
    let extension = MemoriesExtension::default();

    assert!(
        extension
            .tools(
                &ExtensionData::new("session"),
                &ExtensionData::new("thread")
            )
            .is_empty()
    );
}

#[test]
fn tools_are_not_contributed_when_disabled() {
    let extension = MemoriesExtension::default();
    let thread_store = ExtensionData::new("thread");
    thread_store.insert(MemoriesExtensionConfig {
        enabled: false,
        dedicated_tools: true,
        backend: codex_config::types::MemoryBackendKind::Local,
        profile: codex_config::types::MemoryProfile::Personal,
        workspace: "default".to_string(),
        user_peer: "user".to_string(),
        assistant_peer: "codex".to_string(),
        honcho_base_url: None,
        honcho_api_key_env: Some("HONCHO_API_KEY".to_string()),
        write_policy: codex_config::types::MemoryWritePolicy::VisibleTurns,
        sync_policy: codex_config::types::MemorySyncPolicy::Manual,
        cross_profile_policy: codex_config::types::CrossProfilePolicy::DefaultDeny,
        codex_home: test_path_buf("/tmp/codex-home").abs(),
    });

    assert!(
        extension
            .tools(&ExtensionData::new("session"), &thread_store)
            .is_empty()
    );
}

#[test]
fn tools_are_not_contributed_when_dedicated_tools_disabled() {
    let extension = MemoriesExtension::default();
    let thread_store = ExtensionData::new("thread");
    thread_store.insert(MemoriesExtensionConfig {
        enabled: true,
        dedicated_tools: false,
        backend: codex_config::types::MemoryBackendKind::Local,
        profile: codex_config::types::MemoryProfile::Personal,
        workspace: "default".to_string(),
        user_peer: "user".to_string(),
        assistant_peer: "codex".to_string(),
        honcho_base_url: None,
        honcho_api_key_env: Some("HONCHO_API_KEY".to_string()),
        write_policy: codex_config::types::MemoryWritePolicy::VisibleTurns,
        sync_policy: codex_config::types::MemorySyncPolicy::Manual,
        cross_profile_policy: codex_config::types::CrossProfilePolicy::DefaultDeny,
        codex_home: test_path_buf("/tmp/codex-home").abs(),
    });

    assert!(
        extension
            .tools(&ExtensionData::new("session"), &thread_store)
            .is_empty()
    );
}

#[test]
fn tools_are_contributed_when_enabled_with_dedicated_tools() {
    let extension = MemoriesExtension::default();
    let thread_store = ExtensionData::new("thread");
    thread_store.insert(MemoriesExtensionConfig {
        enabled: true,
        dedicated_tools: true,
        backend: codex_config::types::MemoryBackendKind::Local,
        profile: codex_config::types::MemoryProfile::Personal,
        workspace: "default".to_string(),
        user_peer: "user".to_string(),
        assistant_peer: "codex".to_string(),
        honcho_base_url: None,
        honcho_api_key_env: Some("HONCHO_API_KEY".to_string()),
        write_policy: codex_config::types::MemoryWritePolicy::VisibleTurns,
        sync_policy: codex_config::types::MemorySyncPolicy::Manual,
        cross_profile_policy: codex_config::types::CrossProfilePolicy::DefaultDeny,
        codex_home: test_path_buf("/tmp/codex-home").abs(),
    });

    let tool_names = extension
        .tools(&ExtensionData::new("session"), &thread_store)
        .into_iter()
        .map(|tool| tool.tool_name())
        .collect::<Vec<_>>();

    assert_eq!(
        tool_names,
        vec![
            memory_tool_name(crate::ADD_AD_HOC_NOTE_TOOL_NAME),
            memory_tool_name(crate::LIST_TOOL_NAME),
            memory_tool_name(crate::READ_TOOL_NAME),
            memory_tool_name(crate::SEARCH_TOOL_NAME),
        ]
    );
}

#[test]
fn install_registers_dedicated_tool_contributor() {
    let mut builder = ExtensionRegistryBuilder::<codex_core::config::Config>::new();
    crate::install(&mut builder, /*metrics_client*/ None);
    let registry = builder.build();
    let thread_store = ExtensionData::new("thread");
    thread_store.insert(MemoriesExtensionConfig {
        enabled: true,
        dedicated_tools: true,
        backend: codex_config::types::MemoryBackendKind::Local,
        profile: codex_config::types::MemoryProfile::Personal,
        workspace: "default".to_string(),
        user_peer: "user".to_string(),
        assistant_peer: "codex".to_string(),
        honcho_base_url: None,
        honcho_api_key_env: Some("HONCHO_API_KEY".to_string()),
        write_policy: codex_config::types::MemoryWritePolicy::VisibleTurns,
        sync_policy: codex_config::types::MemorySyncPolicy::Manual,
        cross_profile_policy: codex_config::types::CrossProfilePolicy::DefaultDeny,
        codex_home: test_path_buf("/tmp/codex-home").abs(),
    });

    let tool_names = registry
        .tool_contributors()
        .iter()
        .flat_map(|contributor| contributor.tools(&ExtensionData::new("session"), &thread_store))
        .map(|tool| tool.tool_name())
        .collect::<Vec<_>>();

    assert_eq!(
        tool_names,
        vec![
            memory_tool_name(crate::ADD_AD_HOC_NOTE_TOOL_NAME),
            memory_tool_name(crate::LIST_TOOL_NAME),
            memory_tool_name(crate::READ_TOOL_NAME),
            memory_tool_name(crate::SEARCH_TOOL_NAME),
        ]
    );
}

#[test]
fn install_registers_memory_lifecycle_contributors() {
    let mut builder = ExtensionRegistryBuilder::<codex_core::config::Config>::new();
    crate::install(&mut builder, /*metrics_client*/ None);
    let registry = builder.build();

    assert_eq!(registry.thread_lifecycle_contributors().len(), 1);
    assert_eq!(registry.turn_lifecycle_contributors().len(), 1);
    assert_eq!(registry.config_contributors().len(), 1);
    assert_eq!(registry.context_contributors().len(), 1);
    assert_eq!(registry.turn_input_contributors().len(), 1);
    assert_eq!(registry.turn_item_contributors().len(), 1);
    assert_eq!(registry.tool_contributors().len(), 1);
}

#[test]
fn ad_hoc_tool_definition_includes_filename_contract() {
    let tool = memory_tool(
        Path::new("/tmp/codex-home/memories"),
        crate::ADD_AD_HOC_NOTE_TOOL_NAME,
    );
    let spec = serde_json::to_value(tool.spec()).expect("serialize tool spec");

    let filename = spec
        .pointer("/tools/0/parameters/properties/filename")
        .expect("filename parameter should be in tool schema");
    assert_eq!(filename.pointer("/type"), Some(&json!("string")));
    assert!(
        filename
            .pointer("/description")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|description| description.contains("YYYY-MM-DDTHH-MM-SS-<slug>.md"))
    );
}

#[tokio::test]
async fn prompt_contribution_uses_memory_summary_when_enabled() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let memories_dir = tempdir.path().join("memories");
    tokio::fs::create_dir_all(&memories_dir)
        .await
        .expect("create memories dir");
    tokio::fs::write(
        memories_dir.join("memory_summary.md"),
        "Remember repository-specific implementation preferences.",
    )
    .await
    .expect("write memory summary");

    let extension = MemoriesExtension::default();
    let thread_store = ExtensionData::new("thread");
    thread_store.insert(MemoriesExtensionConfig {
        enabled: true,
        dedicated_tools: false,
        backend: codex_config::types::MemoryBackendKind::Local,
        profile: codex_config::types::MemoryProfile::Personal,
        workspace: "default".to_string(),
        user_peer: "user".to_string(),
        assistant_peer: "codex".to_string(),
        honcho_base_url: None,
        honcho_api_key_env: Some("HONCHO_API_KEY".to_string()),
        write_policy: codex_config::types::MemoryWritePolicy::VisibleTurns,
        sync_policy: codex_config::types::MemorySyncPolicy::Manual,
        cross_profile_policy: codex_config::types::CrossProfilePolicy::DefaultDeny,
        codex_home: tempdir.path().abs(),
    });

    let fragments =
        ContextContributor::contribute(&extension, &ExtensionData::new("session"), &thread_store)
            .await;

    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].slot(), PromptSlot::DeveloperPolicy);
    assert!(
        fragments[0]
            .text()
            .contains("Remember repository-specific implementation preferences.")
    );
}

#[tokio::test]
async fn portable_prompt_contribution_describes_provider_without_local_summary() {
    let extension = MemoriesExtension::default();
    let thread_store = ExtensionData::new("thread");
    thread_store.insert(honcho_config(
        codex_config::types::MemoryBackendKind::Honcho,
        "codex-memory-lab",
    ));

    let fragments =
        ContextContributor::contribute(&extension, &ExtensionData::new("session"), &thread_store)
            .await;

    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].slot(), PromptSlot::DeveloperPolicy);
    assert!(fragments[0].text().contains("## Portable Memory"));
    assert!(
        fragments[0]
            .text()
            .contains("Portable memory backend is selected.")
    );
    assert!(
        fragments[0]
            .text()
            .contains("Provider status: not configured.")
    );
    assert!(fragments[0].text().contains("Profile: personal"));
    assert!(fragments[0].text().contains("Workspace: codex-memory-lab"));
}

#[tokio::test]
async fn honcho_recall_is_injected_through_turn_input_contributor() {
    let extension = MemoriesExtension::default();
    let thread_store = ExtensionData::new("thread");
    let turn_store = ExtensionData::new("turn");
    let client = InMemoryHonchoMemoryClient::new();
    client.set_context(HonchoMemoryContext {
        representation: Some("Josh prefers repo-native commands.".to_string()),
        peer_card: vec!["Linux-first workstation.".to_string()],
    });
    thread_store.insert(honcho_config(
        codex_config::types::MemoryBackendKind::Honcho,
        "codex-memory-lab",
    ));
    thread_store.insert(PortableMemoryRuntime::for_provider_tests(
        honcho_settings(
            codex_config::types::MemoryBackendKind::Honcho,
            "codex-memory-lab",
        ),
        crate::honcho::provider_for_tests(
            honcho_settings(
                codex_config::types::MemoryBackendKind::Honcho,
                "codex-memory-lab",
            ),
            client.clone(),
        ),
    ));

    let fragments = TurnInputContributor::contribute(
        &extension,
        TurnInputContext {
            turn_id: "turn-1".to_string(),
            user_input: vec![UserInput::Text {
                text: "inspect memory backend".to_string(),
                text_elements: Vec::new(),
            }],
            environments: Vec::new(),
        },
        &ExtensionData::new("session"),
        &thread_store,
        &turn_store,
    )
    .await;

    assert_eq!(fragments.len(), 1);
    let rendered = fragments[0].render();
    assert!(rendered.contains("<codex_portable_memory"));
    assert!(rendered.contains("Josh prefers repo-native commands."));
    assert!(rendered.contains("Linux-first workstation."));
    assert_eq!(
        client.context_queries(),
        vec!["inspect memory backend".to_string()]
    );
}

#[tokio::test]
async fn missing_honcho_config_fails_open_without_recall() {
    let extension = MemoriesExtension::default();
    let thread_store = ExtensionData::new("thread");
    let turn_store = ExtensionData::new("turn");
    thread_store.insert(honcho_config(
        codex_config::types::MemoryBackendKind::Honcho,
        "",
    ));

    let fragments = TurnInputContributor::contribute(
        &extension,
        TurnInputContext {
            turn_id: "turn-1".to_string(),
            user_input: vec![UserInput::Text {
                text: "need recall".to_string(),
                text_elements: Vec::new(),
            }],
            environments: Vec::new(),
        },
        &ExtensionData::new("session"),
        &thread_store,
        &turn_store,
    )
    .await;

    assert!(fragments.is_empty());
}

#[tokio::test]
async fn visible_turn_writeback_skips_secret_like_content() {
    let extension = MemoriesExtension::default();
    let thread_store = ExtensionData::new("thread");
    let turn_store = ExtensionData::new("turn");
    let client = InMemoryHonchoMemoryClient::new();
    thread_store.insert(honcho_config(
        codex_config::types::MemoryBackendKind::Honcho,
        "codex-memory-lab",
    ));
    thread_store.insert(PortableMemoryRuntime::for_provider_tests(
        honcho_settings(
            codex_config::types::MemoryBackendKind::Honcho,
            "codex-memory-lab",
        ),
        crate::honcho::provider_for_tests(
            honcho_settings(
                codex_config::types::MemoryBackendKind::Honcho,
                "codex-memory-lab",
            ),
            client.clone(),
        ),
    ));

    let mut user_item = TurnItem::UserMessage(UserMessageItem::new(&[UserInput::Text {
        text: "HONCHO_API_KEY=HCH-V3-SECRET-VALUE".to_string(),
        text_elements: Vec::new(),
    }]));
    let mut uppercase_openai_secret =
        TurnItem::UserMessage(UserMessageItem::new(&[UserInput::Text {
            text: "OPENAI_TOKEN=SK-SECRET-VALUE".to_string(),
            text_elements: Vec::new(),
        }]));
    let mut reasoning_item = TurnItem::Reasoning(ReasoningItem {
        id: "reasoning-1".to_string(),
        summary_text: vec!["A visible reasoning summary must not be written.".to_string()],
        raw_content: vec!["hidden chain of thought".to_string()],
    });
    let mut assistant_item = TurnItem::AgentMessage(AgentMessageItem {
        id: "assistant-1".to_string(),
        content: vec![AgentMessageContent::Text {
            text: "I will not store that secret.".to_string(),
        }],
        phase: None,
        memory_citation: None,
    });

    TurnItemContributor::contribute(&extension, &thread_store, &turn_store, &mut user_item)
        .await
        .expect("user item contribution should not fail");
    TurnItemContributor::contribute(
        &extension,
        &thread_store,
        &turn_store,
        &mut uppercase_openai_secret,
    )
    .await
    .expect("uppercase secret item contribution should not fail");
    TurnItemContributor::contribute(&extension, &thread_store, &turn_store, &mut reasoning_item)
        .await
        .expect("reasoning item contribution should not fail");
    TurnItemContributor::contribute(&extension, &thread_store, &turn_store, &mut assistant_item)
        .await
        .expect("assistant item contribution should not fail");
    PortableMemoryRuntime::flush_turn_writeback(&thread_store, &turn_store)
        .await
        .expect("flush should succeed");

    assert_eq!(
        client.messages(),
        vec![HonchoMemoryMessage {
            peer_id: "codex".to_string(),
            content: "I will not store that secret.".to_string(),
            metadata: serde_json::json!({
                "origin": "codex-turn-item",
                "profile": "personal",
                "workspace": "codex-memory-lab",
                "repo": null,
                "sensitivity": "public",
                "portability": "portable",
                "provenance": "visible-assistant-message",
                "confidence": "observed"
            }),
        }]
    );
}

#[tokio::test]
async fn honcho_add_note_rejects_secret_content_without_filename_error() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let memory_root = tempdir.path().join("memories");
    let settings = honcho_settings(
        codex_config::types::MemoryBackendKind::Honcho,
        "codex-memory-lab",
    );
    let backend = SelectedMemoriesBackend::Provider {
        local: LocalMemoriesBackend::from_memory_root(&memory_root),
        provider: crate::honcho::provider_for_tests(settings, InMemoryHonchoMemoryClient::new()),
    };

    let err = crate::backend::MemoriesBackend::add_ad_hoc_note(
        &backend,
        crate::backend::AddAdHocMemoryNoteRequest {
            filename: "2026-06-06T12-00-00-secret-note.md".to_string(),
            note: "OPENAI_TOKEN=SK-SECRET-VALUE".to_string(),
        },
    )
    .await
    .expect_err("secret-like portable note should be rejected");

    assert!(err.to_string().contains("content rejected"));
    assert!(!err.to_string().contains("filename"));
    assert!(
        !memory_root
            .join("extensions/ad_hoc/notes/2026-06-06T12-00-00-secret-note.md")
            .exists()
    );
}

#[tokio::test]
async fn honcho_tool_provider_request_failure_falls_back_to_local() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let memory_root = tempdir.path().join("memories");
    tokio::fs::create_dir_all(&memory_root)
        .await
        .expect("create memory root");
    tokio::fs::write(memory_root.join("MEMORY.md"), "fallback needle\n")
        .await
        .expect("write local memory");
    let backend = SelectedMemoriesBackend::Provider {
        local: LocalMemoriesBackend::from_memory_root(&memory_root),
        provider: Arc::new(FailingMemoryProvider),
    };

    let read = crate::backend::MemoriesBackend::read(
        &backend,
        crate::backend::ReadMemoryRequest {
            path: "MEMORY.md".to_string(),
            line_offset: 1,
            max_lines: None,
            max_tokens: 1024,
        },
    )
    .await
    .expect("provider request failure should fall back to local read");
    assert_eq!(read.content, "fallback needle\n");

    let search = crate::backend::MemoriesBackend::search(
        &backend,
        crate::backend::SearchMemoriesRequest {
            queries: vec!["needle".to_string()],
            match_mode: crate::backend::SearchMatchMode::Any,
            path: None,
            cursor: None,
            context_lines: 0,
            case_sensitive: false,
            normalized: false,
            max_results: 10,
        },
    )
    .await
    .expect("provider request failure should fall back to local search");
    assert_eq!(search.matches.len(), 1);

    crate::backend::MemoriesBackend::add_ad_hoc_note(
        &backend,
        crate::backend::AddAdHocMemoryNoteRequest {
            filename: "2026-06-06T12-00-01-fallback-note.md".to_string(),
            note: "Fallback writes local note when provider is down.".to_string(),
        },
    )
    .await
    .expect("provider request failure should fall back to local note write");
    assert_eq!(
        tokio::fs::read_to_string(
            memory_root
                .join("extensions/ad_hoc/notes")
                .join("2026-06-06T12-00-01-fallback-note.md")
        )
        .await
        .expect("read fallback note"),
        "Fallback writes local note when provider is down."
    );
}

#[test]
fn honcho_loopback_detection_requires_exact_loopback_host() {
    assert!(crate::honcho::is_loopback_url("http://localhost:8000/v3"));
    assert!(crate::honcho::is_loopback_url("http://127.0.0.1:8000/v3"));
    assert!(crate::honcho::is_loopback_url("http://[::1]:8000/v3"));
    assert!(!crate::honcho::is_loopback_url(
        "https://notlocalhost.example/v3"
    ));
    assert!(!crate::honcho::is_loopback_url(
        "https://localhost.example/v3"
    ));
}

#[tokio::test]
async fn hybrid_tool_add_note_keeps_local_cache_and_syncs_provider() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let memory_root = tempdir.path().join("memories");
    let client = InMemoryHonchoMemoryClient::new();
    let settings = honcho_settings(
        codex_config::types::MemoryBackendKind::Hybrid,
        "codex-memory-lab",
    );
    let backend = SelectedMemoriesBackend::Hybrid {
        local: LocalMemoriesBackend::from_memory_root(&memory_root),
        provider: Some(crate::honcho::provider_for_tests(settings, client.clone())),
    };

    crate::backend::MemoriesBackend::add_ad_hoc_note(
        &backend,
        crate::backend::AddAdHocMemoryNoteRequest {
            filename: "2026-06-06T12-00-00-hybrid-note.md".to_string(),
            note: "Hybrid memory keeps a local cache and durable provider copy.".to_string(),
        },
    )
    .await
    .expect("hybrid add note should succeed");

    assert_eq!(
        tokio::fs::read_to_string(
            memory_root
                .join("extensions/ad_hoc/notes")
                .join("2026-06-06T12-00-00-hybrid-note.md")
        )
        .await
        .expect("read local hybrid cache"),
        "Hybrid memory keeps a local cache and durable provider copy."
    );
    assert_eq!(client.conclusions().len(), 1);
    assert_eq!(
        client.conclusions()[0].content,
        "Hybrid memory keeps a local cache and durable provider copy."
    );
}

#[tokio::test]
async fn import_local_preview_reports_bridge_without_provider_write() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let memory_root = tempdir.path().join("memories");
    tokio::fs::create_dir_all(&memory_root)
        .await
        .expect("create memories root");
    tokio::fs::write(
        memory_root.join("MEMORY.md"),
        "Josh prefers repo-native commands and explicit provider boundaries.",
    )
    .await
    .expect("write local memory");

    let settings = honcho_settings(
        codex_config::types::MemoryBackendKind::Honcho,
        "codex-memory-lab",
    );
    let report = crate::import_local::sync_local_codex_memory_with_provider(
        &tempdir.path().abs(),
        &settings,
        crate::import_local::ImportLocalCodexMemoryMode::Preview,
        None,
    )
    .await
    .expect("preview local import");

    assert_eq!(report.endpoint, "/v1/sync/local-codex-memory");
    assert_eq!(report.mode, "preview");
    assert!(!report.provider_configured);
    assert_eq!(report.accepted_files, 1);
    assert_eq!(report.synced_files, 0);
    assert_eq!(report.files[0].path, "MEMORY.md");
    assert_eq!(
        report.files[0].status,
        crate::import_local::ImportLocalCodexMemoryFileStatus::Accepted
    );
    assert!(report.files[0].idempotency_key.is_some());
}

#[tokio::test]
async fn startup_sync_policy_imports_safe_local_memory_files() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let memory_root = tempdir.path().join("memories");
    tokio::fs::create_dir_all(&memory_root)
        .await
        .expect("create memories root");
    tokio::fs::write(
        memory_root.join("MEMORY.md"),
        "Portable memory should preserve upstream local defaults.",
    )
    .await
    .expect("write safe memory");
    tokio::fs::write(
        memory_root.join("secret.md"),
        "HONCHO_API_KEY=hch-v3-secret-value",
    )
    .await
    .expect("write rejected memory");

    let client = InMemoryHonchoMemoryClient::new();
    let mut settings = honcho_settings(
        codex_config::types::MemoryBackendKind::Hybrid,
        "codex-memory-lab",
    );
    settings.sync_policy = codex_config::types::MemorySyncPolicy::Startup;
    let thread_store = ExtensionData::new("thread");
    thread_store.insert(PortableMemoryRuntime::for_provider_tests(
        settings.clone(),
        crate::honcho::provider_for_tests(settings, client.clone()),
    ));

    crate::extension::sync_local_files_on_startup(&thread_store, &tempdir.path().abs()).await;

    let conclusions = client.conclusions();
    assert_eq!(conclusions.len(), 1);
    assert_eq!(
        conclusions[0].content,
        "Portable memory should preserve upstream local defaults."
    );
    assert_eq!(
        conclusions[0].metadata["sync_endpoint"],
        serde_json::json!("/v1/sync/local-codex-memory")
    );
    assert_eq!(
        conclusions[0].metadata["local_path"],
        serde_json::json!("MEMORY.md")
    );
    assert!(
        conclusions[0].metadata["idempotency_key"]
            .as_str()
            .is_some_and(|key| key.starts_with("codex-local-memory:"))
    );
}

#[tokio::test]
async fn manual_sync_policy_does_not_import_on_startup() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let memory_root = tempdir.path().join("memories");
    tokio::fs::create_dir_all(&memory_root)
        .await
        .expect("create memories root");
    tokio::fs::write(
        memory_root.join("MEMORY.md"),
        "Manual sync waits for CLI apply.",
    )
    .await
    .expect("write memory");

    let client = InMemoryHonchoMemoryClient::new();
    let settings = honcho_settings(
        codex_config::types::MemoryBackendKind::Hybrid,
        "codex-memory-lab",
    );
    let thread_store = ExtensionData::new("thread");
    thread_store.insert(PortableMemoryRuntime::for_provider_tests(
        settings.clone(),
        crate::honcho::provider_for_tests(settings, client.clone()),
    ));

    crate::extension::sync_local_files_on_startup(&thread_store, &tempdir.path().abs()).await;

    assert!(client.conclusions().is_empty());
}

#[tokio::test]
async fn add_ad_hoc_note_tool_creates_note_file() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let memory_root = tempdir.path().join("memories");
    let tool = memory_tool(&memory_root, crate::ADD_AD_HOC_NOTE_TOOL_NAME);
    let payload = ToolPayload::Function {
        arguments: json!({
            "filename": "2026-05-26T13-42-08-remember-review-style.md",
            "note": "Remember to keep PR review comments concise.",
        })
        .to_string(),
    };

    let output = tool
        .handle(ToolCall {
            turn_id: "turn-1".to_string(),
            call_id: "call-1".to_string(),
            tool_name: memory_tool_name(crate::ADD_AD_HOC_NOTE_TOOL_NAME),
            model: "gpt-test".to_string(),
            truncation_policy: TruncationPolicy::Bytes(1024),
            conversation_history: codex_extension_api::ConversationHistory::default(),
            turn_item_emitter: Arc::new(NoopTurnItemEmitter),
            payload: payload.clone(),
        })
        .await
        .expect("ad-hoc note should be written");

    assert_eq!(
        output.post_tool_use_response("call-1", &payload),
        Some(json!({}))
    );
    assert_eq!(
        tokio::fs::read_to_string(
            memory_root
                .join("extensions/ad_hoc/notes")
                .join("2026-05-26T13-42-08-remember-review-style.md")
        )
        .await
        .expect("read ad-hoc note"),
        "Remember to keep PR review comments concise."
    );
}

#[tokio::test]
async fn add_ad_hoc_note_tool_rejects_paths_as_filenames() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let memory_root = tempdir.path().join("memories");
    let tool = memory_tool(&memory_root, crate::ADD_AD_HOC_NOTE_TOOL_NAME);
    let payload = ToolPayload::Function {
        arguments: json!({
            "filename": "../2026-05-26T13-42-08-remember-review-style.md",
            "note": "Remember to keep PR review comments concise.",
        })
        .to_string(),
    };

    let result = tool
        .handle(ToolCall {
            turn_id: "turn-1".to_string(),
            call_id: "call-1".to_string(),
            tool_name: memory_tool_name(crate::ADD_AD_HOC_NOTE_TOOL_NAME),
            model: "gpt-test".to_string(),
            truncation_policy: TruncationPolicy::Bytes(1024),
            conversation_history: codex_extension_api::ConversationHistory::default(),
            turn_item_emitter: Arc::new(NoopTurnItemEmitter),
            payload,
        })
        .await;
    let err = match result {
        Ok(_) => panic!("path-like filename should be rejected"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("filename"));
    assert!(err.to_string().contains("YYYY-MM-DDTHH-MM-SS"));
}

#[tokio::test]
async fn read_tool_reads_memory_file() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let memory_root = tempdir.path().join("memories");
    tokio::fs::create_dir_all(&memory_root)
        .await
        .expect("create memories dir");
    tokio::fs::write(
        memory_root.join("MEMORY.md"),
        "first line\nsecond needle line\nthird line\n",
    )
    .await
    .expect("write memory");
    let tool = memory_tool(&memory_root, crate::READ_TOOL_NAME);
    let payload = ToolPayload::Function {
        arguments: json!({
            "path": "MEMORY.md",
            "line_offset": 2,
            "max_lines": 1
        })
        .to_string(),
    };

    let output = tool
        .handle(ToolCall {
            turn_id: "turn-1".to_string(),
            call_id: "call-1".to_string(),
            tool_name: memory_tool_name(crate::READ_TOOL_NAME),
            model: "gpt-test".to_string(),
            truncation_policy: TruncationPolicy::Bytes(1024),
            conversation_history: codex_extension_api::ConversationHistory::default(),
            turn_item_emitter: Arc::new(NoopTurnItemEmitter),
            payload: payload.clone(),
        })
        .await
        .expect("read should succeed");

    assert_eq!(
        output.post_tool_use_response("call-1", &payload),
        Some(json!({
            "path": "MEMORY.md",
            "content": "second needle line\n",
            "start_line_number": 2,
            "truncated": true
        }))
    );
}

#[tokio::test]
async fn search_tool_accepts_multiple_queries() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let memory_root = tempdir.path().join("memories");
    tokio::fs::create_dir_all(&memory_root)
        .await
        .expect("create memories dir");
    tokio::fs::write(
        memory_root.join("MEMORY.md"),
        "alpha only\nneedle only\nalpha needle\n",
    )
    .await
    .expect("write memory");
    let tool = memory_tool(&memory_root, crate::SEARCH_TOOL_NAME);
    let payload = ToolPayload::Function {
        arguments: json!({
            "queries": ["alpha", "needle"],
            "case_sensitive": false
        })
        .to_string(),
    };

    let output = tool
        .handle(ToolCall {
            turn_id: "turn-1".to_string(),
            call_id: "call-1".to_string(),
            tool_name: memory_tool_name(crate::SEARCH_TOOL_NAME),
            model: "gpt-test".to_string(),
            truncation_policy: TruncationPolicy::Bytes(1024),
            conversation_history: codex_extension_api::ConversationHistory::default(),
            turn_item_emitter: Arc::new(NoopTurnItemEmitter),
            payload: payload.clone(),
        })
        .await
        .expect("search should succeed");

    assert_eq!(
        output.post_tool_use_response("call-1", &payload),
        Some(json!({
            "queries": ["alpha", "needle"],
            "match_mode": {
                "type": "any"
            },
            "path": null,
            "matches": [
                {
                    "path": "MEMORY.md",
                    "match_line_number": 1,
                    "content_start_line_number": 1,
                    "content": "alpha only",
                    "matched_queries": ["alpha"]
                },
                {
                    "path": "MEMORY.md",
                    "match_line_number": 2,
                    "content_start_line_number": 2,
                    "content": "needle only",
                    "matched_queries": ["needle"]
                },
                {
                    "path": "MEMORY.md",
                    "match_line_number": 3,
                    "content_start_line_number": 3,
                    "content": "alpha needle",
                    "matched_queries": ["alpha", "needle"]
                }
            ],
            "next_cursor": null,
            "truncated": false
        }))
    );
}

#[tokio::test]
async fn search_tool_accepts_windowed_all_match_mode() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let memory_root = tempdir.path().join("memories");
    tokio::fs::create_dir_all(&memory_root)
        .await
        .expect("create memories dir");
    tokio::fs::write(memory_root.join("MEMORY.md"), "alpha\nmiddle\nneedle\n")
        .await
        .expect("write memory");
    let tool = memory_tool(&memory_root, crate::SEARCH_TOOL_NAME);
    let payload = ToolPayload::Function {
        arguments: json!({
            "queries": ["alpha", "needle"],
            "match_mode": {
                "type": "all_within_lines",
                "line_count": 3
            }
        })
        .to_string(),
    };

    let output = tool
        .handle(ToolCall {
            turn_id: "turn-1".to_string(),
            call_id: "call-1".to_string(),
            tool_name: memory_tool_name(crate::SEARCH_TOOL_NAME),
            model: "gpt-test".to_string(),
            truncation_policy: TruncationPolicy::Bytes(1024),
            conversation_history: codex_extension_api::ConversationHistory::default(),
            turn_item_emitter: Arc::new(NoopTurnItemEmitter),
            payload: payload.clone(),
        })
        .await
        .expect("search should succeed");

    assert_eq!(
        output.post_tool_use_response("call-1", &payload),
        Some(json!({
            "queries": ["alpha", "needle"],
            "match_mode": {
                "type": "all_within_lines",
                "line_count": 3
            },
            "path": null,
            "matches": [
                {
                    "path": "MEMORY.md",
                    "match_line_number": 1,
                    "content_start_line_number": 1,
                    "content": "alpha\nmiddle\nneedle",
                    "matched_queries": ["alpha", "needle"]
                }
            ],
            "next_cursor": null,
            "truncated": false
        }))
    );
}

#[tokio::test]
async fn search_tool_rejects_legacy_single_query() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let memory_root = tempdir.path().join("memories");
    tokio::fs::create_dir_all(&memory_root)
        .await
        .expect("create memories dir");
    let tool = memory_tool(&memory_root, crate::SEARCH_TOOL_NAME);
    let payload = ToolPayload::Function {
        arguments: json!({
            "query": "needle",
        })
        .to_string(),
    };

    let result = tool
        .handle(ToolCall {
            turn_id: "turn-1".to_string(),
            call_id: "call-1".to_string(),
            tool_name: memory_tool_name(crate::SEARCH_TOOL_NAME),
            model: "gpt-test".to_string(),
            truncation_policy: TruncationPolicy::Bytes(1024),
            conversation_history: codex_extension_api::ConversationHistory::default(),
            turn_item_emitter: Arc::new(NoopTurnItemEmitter),
            payload,
        })
        .await;
    let err = match result {
        Ok(_) => panic!("legacy query field should be rejected"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("unknown field"));
    assert!(err.to_string().contains("query"));
}

fn memory_tool(memory_root: &Path, tool_name: &str) -> Arc<dyn ToolExecutor<ToolCall>> {
    let expected_tool_name = memory_tool_name(tool_name);
    crate::tools::memory_tools(
        LocalMemoriesBackend::from_memory_root(memory_root),
        /*metrics_client*/ None,
    )
    .into_iter()
    .find(|tool| tool.tool_name() == expected_tool_name)
    .unwrap_or_else(|| panic!("{tool_name} tool should be registered"))
}

fn memory_tool_name(tool_name: &str) -> ToolName {
    ToolName::namespaced(crate::MEMORY_TOOLS_NAMESPACE, tool_name)
}

fn honcho_config(
    backend: codex_config::types::MemoryBackendKind,
    workspace: &str,
) -> MemoriesExtensionConfig {
    MemoriesExtensionConfig {
        enabled: true,
        dedicated_tools: true,
        backend,
        profile: codex_config::types::MemoryProfile::Personal,
        workspace: workspace.to_string(),
        user_peer: "user".to_string(),
        assistant_peer: "codex".to_string(),
        honcho_base_url: None,
        honcho_api_key_env: Some("HONCHO_API_KEY".to_string()),
        write_policy: codex_config::types::MemoryWritePolicy::VisibleTurns,
        sync_policy: codex_config::types::MemorySyncPolicy::Manual,
        cross_profile_policy: codex_config::types::CrossProfilePolicy::DefaultDeny,
        codex_home: test_path_buf("/tmp/codex-home").abs(),
    }
}

fn honcho_settings(
    backend: codex_config::types::MemoryBackendKind,
    workspace: &str,
) -> PortableMemorySettings {
    PortableMemorySettings {
        backend,
        profile: codex_config::types::MemoryProfile::Personal,
        workspace: workspace.to_string(),
        user_peer: "user".to_string(),
        assistant_peer: "codex".to_string(),
        honcho_base_url: None,
        honcho_api_key_env: Some("HONCHO_API_KEY".to_string()),
        write_policy: codex_config::types::MemoryWritePolicy::VisibleTurns,
        sync_policy: codex_config::types::MemorySyncPolicy::Manual,
        cross_profile_policy: codex_config::types::CrossProfilePolicy::DefaultDeny,
    }
}

#[derive(Clone)]
struct FailingMemoryProvider;

impl MemoryProvider for FailingMemoryProvider {
    fn recall(&self, _query: String) -> ProviderFuture<'_, PortableMemoryContext> {
        provider_unavailable()
    }

    fn search(
        &self,
        _request: crate::backend::SearchMemoriesRequest,
    ) -> ProviderFuture<'_, crate::backend::SearchMemoriesResponse> {
        provider_unavailable()
    }

    fn list(
        &self,
        _request: crate::backend::ListMemoriesRequest,
    ) -> ProviderFuture<'_, crate::backend::ListMemoriesResponse> {
        provider_unavailable()
    }

    fn read(
        &self,
        _request: crate::backend::ReadMemoryRequest,
    ) -> ProviderFuture<'_, crate::backend::ReadMemoryResponse> {
        provider_unavailable()
    }

    fn add_note(
        &self,
        _request: crate::backend::AddAdHocMemoryNoteRequest,
    ) -> ProviderFuture<'_, crate::backend::AddAdHocMemoryNoteResponse> {
        provider_unavailable()
    }

    fn write_visible_turn(&self, _messages: Vec<VisibleMemoryMessage>) -> ProviderFuture<'_, ()> {
        provider_unavailable()
    }

    fn conclude(&self, _conclusion: PortableMemoryConclusion) -> ProviderFuture<'_, ()> {
        provider_unavailable()
    }

    fn sync_local_files(
        &self,
        _request: LocalCodexMemorySyncRequest,
    ) -> ProviderFuture<'_, LocalCodexMemorySyncResponse> {
        provider_unavailable()
    }
}

fn provider_unavailable<T>() -> ProviderFuture<'static, T> {
    Box::pin(async {
        Err(PortableMemoryError::Request(
            "provider unavailable".to_string(),
        ))
    })
}
