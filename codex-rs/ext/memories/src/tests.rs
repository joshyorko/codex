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
        provider: codex_config::types::MemoryProviderKind::Honcho,
        profile: codex_config::types::MemoryProfile::Personal,
        workspace: "default".to_string(),
        user_peer: "user".to_string(),
        assistant_peer: "codex".to_string(),
        provider_url: None,
        honcho_base_url: None,
        honcho_api_key_env: Some("HONCHO_API_KEY".to_string()),
        write_policy: codex_config::types::MemoryWritePolicy::VisibleTurns,
        sync_policy: codex_config::types::MemorySyncPolicy::Manual,
        local_import_policy: codex_config::types::LocalImportPolicy::Manual,
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
        provider: codex_config::types::MemoryProviderKind::Honcho,
        profile: codex_config::types::MemoryProfile::Personal,
        workspace: "default".to_string(),
        user_peer: "user".to_string(),
        assistant_peer: "codex".to_string(),
        provider_url: None,
        honcho_base_url: None,
        honcho_api_key_env: Some("HONCHO_API_KEY".to_string()),
        write_policy: codex_config::types::MemoryWritePolicy::VisibleTurns,
        sync_policy: codex_config::types::MemorySyncPolicy::Manual,
        local_import_policy: codex_config::types::LocalImportPolicy::Manual,
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
        provider: codex_config::types::MemoryProviderKind::Honcho,
        profile: codex_config::types::MemoryProfile::Personal,
        workspace: "default".to_string(),
        user_peer: "user".to_string(),
        assistant_peer: "codex".to_string(),
        provider_url: None,
        honcho_base_url: None,
        honcho_api_key_env: Some("HONCHO_API_KEY".to_string()),
        write_policy: codex_config::types::MemoryWritePolicy::VisibleTurns,
        sync_policy: codex_config::types::MemorySyncPolicy::Manual,
        local_import_policy: codex_config::types::LocalImportPolicy::Manual,
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
        provider: codex_config::types::MemoryProviderKind::Honcho,
        profile: codex_config::types::MemoryProfile::Personal,
        workspace: "default".to_string(),
        user_peer: "user".to_string(),
        assistant_peer: "codex".to_string(),
        provider_url: None,
        honcho_base_url: None,
        honcho_api_key_env: Some("HONCHO_API_KEY".to_string()),
        write_policy: codex_config::types::MemoryWritePolicy::VisibleTurns,
        sync_policy: codex_config::types::MemorySyncPolicy::Manual,
        local_import_policy: codex_config::types::LocalImportPolicy::Manual,
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
        provider: codex_config::types::MemoryProviderKind::Honcho,
        profile: codex_config::types::MemoryProfile::Personal,
        workspace: "default".to_string(),
        user_peer: "user".to_string(),
        assistant_peer: "codex".to_string(),
        provider_url: None,
        honcho_base_url: None,
        honcho_api_key_env: Some("HONCHO_API_KEY".to_string()),
        write_policy: codex_config::types::MemoryWritePolicy::VisibleTurns,
        sync_policy: codex_config::types::MemorySyncPolicy::Manual,
        local_import_policy: codex_config::types::LocalImportPolicy::Manual,
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
        codex_config::types::MemoryBackendKind::Provider,
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
        codex_config::types::MemoryBackendKind::Provider,
        "codex-memory-lab",
    ));
    thread_store.insert(PortableMemoryRuntime::for_provider_tests(
        honcho_settings(
            codex_config::types::MemoryBackendKind::Provider,
            "codex-memory-lab",
        ),
        crate::honcho::provider_for_tests(
            honcho_settings(
                codex_config::types::MemoryBackendKind::Provider,
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
        codex_config::types::MemoryBackendKind::Provider,
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

#[test]
fn missing_codex_memoryd_url_falls_back_to_local_backend() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let mut settings = honcho_settings(
        codex_config::types::MemoryBackendKind::Provider,
        "codex-memory-lab",
    );
    settings.provider = codex_config::types::MemoryProviderKind::CodexMemoryd;
    settings.provider_url = None;

    let selected = SelectedMemoriesBackend::from_settings(
        LocalMemoriesBackend::from_memory_root(tempdir.path().join("memories")),
        settings,
    );

    assert!(matches!(selected, SelectedMemoriesBackend::Local(_)));
}

#[tokio::test]
async fn visible_turn_writeback_skips_secret_like_content() {
    let extension = MemoriesExtension::default();
    let thread_store = ExtensionData::new("thread");
    let turn_store = ExtensionData::new("turn");
    let client = InMemoryHonchoMemoryClient::new();
    thread_store.insert(honcho_config(
        codex_config::types::MemoryBackendKind::Provider,
        "codex-memory-lab",
    ));
    thread_store.insert(PortableMemoryRuntime::for_provider_tests(
        honcho_settings(
            codex_config::types::MemoryBackendKind::Provider,
            "codex-memory-lab",
        ),
        crate::honcho::provider_for_tests(
            honcho_settings(
                codex_config::types::MemoryBackendKind::Provider,
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
        codex_config::types::MemoryBackendKind::Provider,
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
        codex_config::types::MemoryBackendKind::Provider,
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
async fn import_local_preview_accepts_large_files_by_default() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let memory_root = tempdir.path().join("memories");
    tokio::fs::create_dir_all(&memory_root)
        .await
        .expect("create memories root");
    let large_memory = "Josh prefers repo-native commands.\n".repeat(4096);
    tokio::fs::write(memory_root.join("MEMORY.md"), large_memory.as_bytes())
        .await
        .expect("write large local memory");

    let settings = honcho_settings(
        codex_config::types::MemoryBackendKind::Provider,
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

    assert_eq!(report.accepted_files, 1);
    assert_eq!(report.rejected_files, 0);
    assert_eq!(report.files[0].path, "MEMORY.md");
    assert_eq!(report.files[0].bytes, large_memory.len() as u64);
    assert_eq!(
        report.files[0].status,
        crate::import_local::ImportLocalCodexMemoryFileStatus::Accepted
    );
}

#[tokio::test]
async fn import_local_preview_redacts_blocked_lines_instead_of_rejecting_file() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let memory_root = tempdir.path().join("memories");
    tokio::fs::create_dir_all(&memory_root)
        .await
        .expect("create memories root");
    tokio::fs::write(
        memory_root.join("MEMORY.md"),
        "Josh prefers repo-native commands.\npassword=do-not-import\nKeep local imports useful.",
    )
    .await
    .expect("write mixed local memory");

    let settings = honcho_settings(
        codex_config::types::MemoryBackendKind::Provider,
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

    assert_eq!(report.accepted_files, 1);
    assert_eq!(report.rejected_files, 0);
    assert_eq!(report.files[0].path, "MEMORY.md");
    assert_eq!(
        report.files[0].status,
        crate::import_local::ImportLocalCodexMemoryFileStatus::Accepted
    );
}

#[test]
fn local_import_sanitizer_drops_blocked_lines() {
    let content = crate::policy::sanitize_local_import_memory_content(
        "Keep this.\npassword=do-not-import\nKeep this too.",
    )
    .expect("mixed local memory should sanitize");

    assert_eq!(content, "Keep this.\nKeep this too.");
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
    settings.local_import_policy = codex_config::types::LocalImportPolicy::StartupApply;
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
            environments: Vec::new(),
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
            environments: Vec::new(),
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
            environments: Vec::new(),
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
            environments: Vec::new(),
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
            environments: Vec::new(),
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
            environments: Vec::new(),
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
        provider: codex_config::types::MemoryProviderKind::Honcho,
        profile: codex_config::types::MemoryProfile::Personal,
        workspace: workspace.to_string(),
        user_peer: "user".to_string(),
        assistant_peer: "codex".to_string(),
        provider_url: None,
        honcho_base_url: None,
        honcho_api_key_env: Some("HONCHO_API_KEY".to_string()),
        write_policy: codex_config::types::MemoryWritePolicy::VisibleTurns,
        sync_policy: codex_config::types::MemorySyncPolicy::Manual,
        local_import_policy: codex_config::types::LocalImportPolicy::Manual,
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
        provider: codex_config::types::MemoryProviderKind::Honcho,
        profile: codex_config::types::MemoryProfile::Personal,
        workspace: workspace.to_string(),
        user_peer: "user".to_string(),
        assistant_peer: "codex".to_string(),
        provider_url: None,
        honcho_base_url: None,
        honcho_api_key_env: Some("HONCHO_API_KEY".to_string()),
        write_policy: codex_config::types::MemoryWritePolicy::VisibleTurns,
        sync_policy: codex_config::types::MemorySyncPolicy::Manual,
        local_import_policy: codex_config::types::LocalImportPolicy::Manual,
        cross_profile_policy: codex_config::types::CrossProfilePolicy::DefaultDeny,
    }
}

fn codex_memoryd_settings(
    backend: codex_config::types::MemoryBackendKind,
    workspace: &str,
    provider_url: Option<String>,
) -> PortableMemorySettings {
    let mut settings = honcho_settings(backend, workspace);
    settings.provider = codex_config::types::MemoryProviderKind::CodexMemoryd;
    settings.provider_url = provider_url;
    settings
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

mod provider_conformance {
    use codex_extension_api::ExtensionData;
    use codex_protocol::items::AgentMessageContent;
    use codex_protocol::items::AgentMessageItem;
    use codex_protocol::items::TurnItem;
    use codex_protocol::items::UserMessageItem;
    use codex_protocol::user_input::UserInput;
    use codex_utils_absolute_path::test_support::PathExt;

    use super::InMemoryHonchoMemoryClient;
    use super::LocalMemoriesBackend;
    use super::PortableMemoryRuntime;
    use super::SelectedMemoriesBackend;
    use super::codex_memoryd_settings;
    use super::honcho_settings;
    use crate::backend::AddAdHocMemoryNoteRequest;
    use crate::backend::ListMemoriesRequest;
    use crate::backend::MemoriesBackend;
    use crate::backend::MemoriesBackendError;
    use crate::backend::ReadMemoryRequest;
    use crate::backend::SearchMatchMode;
    use crate::backend::SearchMemoriesRequest;
    use crate::codex_memoryd::serve_scripted;

    #[tokio::test]
    async fn local_list_sorting_cursor_and_invalid_cursor() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let memory_root = tempdir.path().join("memories");
        tokio::fs::create_dir_all(&memory_root)
            .await
            .expect("create memory root");
        tokio::fs::create_dir_all(memory_root.join("notes"))
            .await
            .expect("create memory directory");
        tokio::fs::write(memory_root.join("zeta.md"), "zeta\n")
            .await
            .expect("write zeta");
        tokio::fs::write(memory_root.join("alpha.md"), "alpha\n")
            .await
            .expect("write alpha");
        tokio::fs::write(memory_root.join("notes").join("inner.md"), "inner\n")
            .await
            .expect("write inner note");

        let backend = LocalMemoriesBackend::from_memory_root(&memory_root);

        let first = MemoriesBackend::list(
            &backend,
            ListMemoriesRequest {
                path: None,
                cursor: None,
                max_results: 2,
            },
        )
        .await
        .expect("list with cursor should succeed");
        assert_eq!(
            first.entries,
            vec![
                crate::backend::MemoryEntry {
                    path: "alpha.md".to_string(),
                    entry_type: crate::backend::MemoryEntryType::File,
                },
                crate::backend::MemoryEntry {
                    path: "notes".to_string(),
                    entry_type: crate::backend::MemoryEntryType::Directory,
                },
            ]
        );
        assert_eq!(first.next_cursor, Some("2".to_string()));
        assert!(first.truncated);

        let second = MemoriesBackend::list(
            &backend,
            ListMemoriesRequest {
                path: None,
                cursor: Some("2".to_string()),
                max_results: 2,
            },
        )
        .await
        .expect("paginated list should succeed");
        assert_eq!(second.entries.len(), 1);
        assert_eq!(second.entries[0].path, "zeta.md");
        assert!(!second.truncated);

        let err = MemoriesBackend::list(
            &backend,
            ListMemoriesRequest {
                path: None,
                cursor: Some("10".to_string()),
                max_results: 2,
            },
        )
        .await
        .expect_err("invalid cursor should be rejected");
        assert!(matches!(
            err,
            MemoriesBackendError::InvalidCursor {
                cursor,
                reason
            } if cursor == "10" && reason == "exceeds result count"
        ));
    }

    #[tokio::test]
    async fn local_read_validation_rejects_bad_offsets_and_missing_file() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let memory_root = tempdir.path().join("memories");
        tokio::fs::create_dir_all(&memory_root)
            .await
            .expect("create memory root");
        tokio::fs::write(memory_root.join("MEMORY.md"), "alpha\nbeta\n")
            .await
            .expect("write memory file");

        let backend = LocalMemoriesBackend::from_memory_root(&memory_root);

        let invalid_line_offset = MemoriesBackend::read(
            &backend,
            ReadMemoryRequest {
                path: "MEMORY.md".to_string(),
                line_offset: 0,
                max_lines: Some(1),
                max_tokens: 1024,
            },
        )
        .await
        .expect_err("line_offset=0 should be invalid");
        assert!(matches!(
            invalid_line_offset,
            MemoriesBackendError::InvalidLineOffset
        ));

        let invalid_max_lines = MemoriesBackend::read(
            &backend,
            ReadMemoryRequest {
                path: "MEMORY.md".to_string(),
                line_offset: 1,
                max_lines: Some(0),
                max_tokens: 1024,
            },
        )
        .await
        .expect_err("max_lines=0 should be invalid");
        assert!(matches!(
            invalid_max_lines,
            MemoriesBackendError::InvalidMaxLines
        ));

        let missing_file = MemoriesBackend::read(
            &backend,
            ReadMemoryRequest {
                path: "missing.md".to_string(),
                line_offset: 1,
                max_lines: Some(1),
                max_tokens: 1024,
            },
        )
        .await
        .expect_err("missing file should be rejected");
        assert!(matches!(
            missing_file,
            MemoriesBackendError::NotFound { path } if path == "missing.md"
        ));

        let out_of_range = MemoriesBackend::read(
            &backend,
            ReadMemoryRequest {
                path: "MEMORY.md".to_string(),
                line_offset: 99,
                max_lines: Some(1),
                max_tokens: 1024,
            },
        )
        .await
        .expect_err("out-of-range line_offset should be rejected");
        assert!(matches!(
            out_of_range,
            MemoriesBackendError::LineOffsetExceedsFileLength
        ));
    }

    #[tokio::test]
    async fn local_search_validation_and_window_ordering() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let memory_root = tempdir.path().join("memories");
        tokio::fs::create_dir_all(&memory_root)
            .await
            .expect("create memory root");
        tokio::fs::write(memory_root.join("alpha.md"), "alpha\nbridge\ngamma\n")
            .await
            .expect("write alpha");
        tokio::fs::write(memory_root.join("zeta.md"), "zeta\nafter\n")
            .await
            .expect("write zeta");

        let backend = LocalMemoriesBackend::from_memory_root(&memory_root);

        let empty_query = MemoriesBackend::search(
            &backend,
            SearchMemoriesRequest {
                queries: vec![],
                match_mode: SearchMatchMode::Any,
                path: None,
                cursor: None,
                context_lines: 0,
                case_sensitive: false,
                normalized: false,
                max_results: 10,
            },
        )
        .await
        .expect_err("empty query should be rejected");
        assert!(matches!(empty_query, MemoriesBackendError::EmptyQuery));

        let bad_window = MemoriesBackend::search(
            &backend,
            SearchMemoriesRequest {
                queries: vec!["alpha".to_string()],
                match_mode: SearchMatchMode::AllWithinLines { line_count: 0 },
                path: None,
                cursor: None,
                context_lines: 0,
                case_sensitive: false,
                normalized: false,
                max_results: 10,
            },
        )
        .await
        .expect_err("invalid match window should be rejected");
        assert!(matches!(
            bad_window,
            MemoriesBackendError::InvalidMatchWindow
        ));

        let any = MemoriesBackend::search(
            &backend,
            SearchMemoriesRequest {
                queries: vec!["alpha".to_string()],
                match_mode: SearchMatchMode::Any,
                path: None,
                cursor: None,
                context_lines: 0,
                case_sensitive: false,
                normalized: false,
                max_results: 10,
            },
        )
        .await
        .expect("search should succeed");
        assert_eq!(
            any.matches
                .into_iter()
                .map(|entry| entry.path)
                .collect::<Vec<_>>(),
            vec!["alpha.md"]
        );

        let _windowed = MemoriesBackend::search(
            &backend,
            SearchMemoriesRequest {
                queries: vec!["alpha".to_string(), "gamma".to_string()],
                match_mode: SearchMatchMode::AllWithinLines { line_count: 3 },
                path: None,
                cursor: None,
                context_lines: 0,
                case_sensitive: false,
                normalized: false,
                max_results: 10,
            },
        )
        .await
        .expect("windowed search should return matches");
    }

    #[tokio::test]
    async fn provider_empty_query_rejected_without_network_for_honcho_and_memoryd() {
        let memory_root = tempfile::tempdir()
            .expect("tempdir")
            .path()
            .join("memories");
        let honcho_client = InMemoryHonchoMemoryClient::new();
        let honcho = crate::honcho::provider_for_tests(
            honcho_settings(
                codex_config::types::MemoryBackendKind::Provider,
                "codex-memory-lab",
            ),
            honcho_client,
        );
        let honcho_backend = SelectedMemoriesBackend::Provider {
            local: LocalMemoriesBackend::from_memory_root(&memory_root),
            provider: honcho,
        };
        let honcho_err = MemoriesBackend::search(
            &honcho_backend,
            SearchMemoriesRequest {
                queries: vec![],
                match_mode: SearchMatchMode::Any,
                path: None,
                cursor: None,
                context_lines: 0,
                case_sensitive: false,
                normalized: false,
                max_results: 10,
            },
        )
        .await
        .expect_err("honcho should reject empty query before client usage");
        assert!(matches!(honcho_err, MemoriesBackendError::EmptyQuery));

        let memoryd_backend = SelectedMemoriesBackend::from_settings(
            LocalMemoriesBackend::from_memory_root(&memory_root),
            codex_memoryd_settings(
                codex_config::types::MemoryBackendKind::Provider,
                "codex-memory-lab",
                Some("http://127.0.0.1:65535".to_string()),
            ),
        );
        let memoryd_err = MemoriesBackend::search(
            &memoryd_backend,
            SearchMemoriesRequest {
                queries: vec![],
                match_mode: SearchMatchMode::Any,
                path: None,
                cursor: None,
                context_lines: 0,
                case_sensitive: false,
                normalized: false,
                max_results: 10,
            },
        )
        .await
        .expect_err("codex_memoryd should reject empty query before client usage");
        assert!(matches!(memoryd_err, MemoriesBackendError::EmptyQuery));
    }

    #[tokio::test]
    async fn codex_memoryd_provider_search_fans_out_and_maps_matches() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let memory_root = tempdir.path().join("memories");
        tokio::fs::create_dir_all(&memory_root)
            .await
            .expect("create memory root");
        let (base_url, request_rx, server) = serve_scripted(vec![
            (
                "200 OK",
                r#"{"ok":true,"data":{"matches":[{"path":"alpha.md","content":"alpha hit"}]}}"#,
            ),
            (
                "200 OK",
                r#"{"ok":true,"data":{"matches":[{"path":"needle.md","content":"needle hit"}]}}"#,
            ),
        ]);
        let backend = SelectedMemoriesBackend::from_settings(
            LocalMemoriesBackend::from_memory_root(&memory_root),
            codex_memoryd_settings(
                codex_config::types::MemoryBackendKind::Provider,
                "codex-memory-lab",
                Some(base_url),
            ),
        );

        let search = MemoriesBackend::search(
            &backend,
            SearchMemoriesRequest {
                queries: vec!["alpha".to_string(), "needle".to_string()],
                match_mode: SearchMatchMode::Any,
                path: None,
                cursor: None,
                context_lines: 0,
                case_sensitive: false,
                normalized: false,
                max_results: 10,
            },
        )
        .await
        .expect("codex_memoryd search should succeed");

        let first_request = request_rx.recv().expect("first search request");
        let second_request = request_rx.recv().expect("second search request");
        server.join().expect("server thread should finish");

        assert!(first_request.starts_with("POST /v1/search "));
        assert!(first_request.contains(r#""query":"alpha""#));
        assert!(second_request.starts_with("POST /v1/search "));
        assert!(second_request.contains(r#""query":"needle""#));
        assert_eq!(
            search.matches,
            vec![
                crate::backend::MemorySearchMatch {
                    path: "alpha.md".to_string(),
                    match_line_number: 1,
                    content_start_line_number: 1,
                    content: "alpha hit".to_string(),
                    matched_queries: vec!["alpha".to_string()],
                },
                crate::backend::MemorySearchMatch {
                    path: "needle.md".to_string(),
                    match_line_number: 2,
                    content_start_line_number: 2,
                    content: "needle hit".to_string(),
                    matched_queries: vec!["needle".to_string()],
                },
            ]
        );
        assert_eq!(search.queries, vec!["alpha", "needle"]);
        assert_eq!(search.match_mode, SearchMatchMode::Any);
        assert!(!search.truncated);
        assert_eq!(search.next_cursor, None);
    }

    #[tokio::test]
    async fn codex_memoryd_provider_list_and_read_portable_status_round_trip() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let memory_root = tempdir.path().join("memories");
        tokio::fs::create_dir_all(&memory_root)
            .await
            .expect("create memory root");
        let (base_url, request_rx, server) = serve_scripted(vec![(
            "200 OK",
            r#"{"ok":true,"data":{"state":"ready","summary":"portable memory status"}}"#,
        )]);
        let backend = SelectedMemoriesBackend::from_settings(
            LocalMemoriesBackend::from_memory_root(&memory_root),
            codex_memoryd_settings(
                codex_config::types::MemoryBackendKind::Provider,
                "codex-memory-lab",
                Some(base_url),
            ),
        );

        let list = MemoriesBackend::list(
            &backend,
            ListMemoriesRequest {
                path: None,
                cursor: None,
                max_results: 10,
            },
        )
        .await
        .expect("codex_memoryd list should succeed");
        assert_eq!(list.path, None);
        assert_eq!(
            list.entries,
            vec![crate::backend::MemoryEntry {
                path: "portable/status.md".to_string(),
                entry_type: crate::backend::MemoryEntryType::File,
            }]
        );

        let read = MemoriesBackend::read(
            &backend,
            ReadMemoryRequest {
                path: "portable/status.md".to_string(),
                line_offset: 1,
                max_lines: Some(10),
                max_tokens: 1024,
            },
        )
        .await
        .expect("codex_memoryd read should succeed");

        let request = request_rx.recv().expect("status request");
        server.join().expect("server thread should finish");

        assert!(request.starts_with("GET /v1/status "));
        assert_eq!(read.path, "portable/status.md");
        assert_eq!(read.start_line_number, 1);
        assert!(!read.truncated);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&read.content).expect("pretty-printed JSON"),
            serde_json::json!({
                "state": "ready",
                "summary": "portable memory status"
            })
        );
    }

    #[tokio::test]
    async fn codex_memoryd_provider_add_note_posts_conclusion_payload() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let memory_root = tempdir.path().join("memories");
        tokio::fs::create_dir_all(&memory_root)
            .await
            .expect("create memory root");
        let (base_url, request_rx, server) =
            serve_scripted(vec![("200 OK", r#"{"ok":true,"data":{}}"#)]);
        let backend = SelectedMemoriesBackend::from_settings(
            LocalMemoriesBackend::from_memory_root(&memory_root),
            codex_memoryd_settings(
                codex_config::types::MemoryBackendKind::Provider,
                "codex-memory-lab",
                Some(base_url),
            ),
        );

        crate::backend::MemoriesBackend::add_ad_hoc_note(
            &backend,
            AddAdHocMemoryNoteRequest {
                filename: "2026-06-12T12-00-00-codex-memoryd-note.md".to_string(),
                note: "Remember repo-native commands.".to_string(),
            },
        )
        .await
        .expect("codex_memoryd add note should succeed");

        let request = request_rx.recv().expect("conclusion request");
        server.join().expect("server thread should finish");

        assert!(request.starts_with("POST /v1/conclusions "));
        assert!(request.contains(r#""target":"user""#));
        assert!(request.contains(r#""conclusions":["Remember repo-native commands."]"#));
        assert!(request.contains(r#""filename":"2026-06-12T12-00-00-codex-memoryd-note.md""#));
    }

    #[tokio::test]
    async fn codex_memoryd_hybrid_add_note_keeps_local_cache_when_provider_errors() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let memory_root = tempdir.path().join("memories");
        tokio::fs::create_dir_all(&memory_root)
            .await
            .expect("create memory root");
        let (base_url, request_rx, server) = serve_scripted(vec![(
            "500 Internal Server Error",
            r#"{"ok":false,"error":{"message":"provider down"}}"#,
        )]);
        let backend = SelectedMemoriesBackend::from_settings(
            LocalMemoriesBackend::from_memory_root(&memory_root),
            codex_memoryd_settings(
                codex_config::types::MemoryBackendKind::Hybrid,
                "codex-memory-lab",
                Some(base_url),
            ),
        );

        crate::backend::MemoriesBackend::add_ad_hoc_note(
            &backend,
            AddAdHocMemoryNoteRequest {
                filename: "2026-06-12T12-00-01-codex-memoryd-hybrid-note.md".to_string(),
                note: "Hybrid memory keeps a local cache.".to_string(),
            },
        )
        .await
        .expect("hybrid add note should stay local-first");

        let request = request_rx.recv().expect("hybrid provider request");
        server.join().expect("server thread should finish");

        assert!(request.starts_with("POST /v1/conclusions "));
        assert_eq!(
            tokio::fs::read_to_string(
                memory_root
                    .join("extensions/ad_hoc/notes")
                    .join("2026-06-12T12-00-01-codex-memoryd-hybrid-note.md")
            )
            .await
            .expect("read local hybrid cache"),
            "Hybrid memory keeps a local cache."
        );
    }

    #[tokio::test]
    async fn codex_memoryd_runtime_writeback_and_local_import_use_expected_endpoints() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let memory_root = tempdir.path().join("memories");
        tokio::fs::create_dir_all(&memory_root)
            .await
            .expect("create memory root");
        tokio::fs::write(
            memory_root.join("MEMORY.md"),
            "Portable memory keeps repo-native defaults.\n",
        )
        .await
        .expect("write local memory");
        let (base_url, request_rx, server) = serve_scripted(vec![
            ("200 OK", r#"{"ok":true,"data":{}}"#),
            (
                "200 OK",
                r#"{"ok":true,"data":{"created":1,"updated":0,"proposed":0,"skipped":0,"rejected":0}}"#,
            ),
        ]);
        let settings = codex_memoryd_settings(
            codex_config::types::MemoryBackendKind::Provider,
            "codex-memory-lab",
            Some(base_url),
        );
        let provider =
            crate::codex_memoryd::provider_from_settings(&settings).expect("provider should exist");
        let runtime = PortableMemoryRuntime::for_provider_tests(settings.clone(), provider);
        let thread_store = ExtensionData::new("thread");
        let turn_store = ExtensionData::new("turn");
        thread_store.insert(runtime);

        let user_item = TurnItem::UserMessage(UserMessageItem::new(&[UserInput::Text {
            text: "Remember repo-native commands.".to_string(),
            text_elements: Vec::new(),
        }]));
        let assistant_item = TurnItem::AgentMessage(AgentMessageItem {
            id: "assistant-1".to_string(),
            content: vec![AgentMessageContent::Text {
                text: "Keep the local import preview safe.".to_string(),
            }],
            phase: None,
            memory_citation: None,
        });

        let runtime = thread_store
            .get::<PortableMemoryRuntime>()
            .expect("runtime should be stored");
        runtime
            .record_turn_item(&turn_store, &user_item)
            .expect("user turn item should record");
        runtime
            .record_turn_item(&turn_store, &assistant_item)
            .expect("assistant turn item should record");
        PortableMemoryRuntime::flush_turn_writeback(&thread_store, &turn_store)
            .await
            .expect("flush should succeed");
        let report = runtime
            .sync_local_files(
                &tempdir.path().abs(),
                crate::import_local::ImportLocalCodexMemoryMode::Preview,
            )
            .await
            .expect("local sync should succeed");

        let writeback_request = request_rx.recv().expect("turn writeback request");
        let sync_request = request_rx.recv().expect("local import request");
        server.join().expect("server thread should finish");

        assert!(writeback_request.starts_with("POST /v1/turns "));
        assert!(writeback_request.contains(r#""write_policy":"visible_turns""#));
        assert!(writeback_request.contains("Remember repo-native commands."));
        assert!(writeback_request.contains("Keep the local import preview safe."));
        assert!(sync_request.starts_with("POST /v1/sync/local-codex-memory "));
        assert!(sync_request.contains(r#""mode":"preview""#));
        assert!(sync_request.contains(r#""path":"MEMORY.md""#));
        assert!(sync_request.contains(r#""kind":"memory_registry""#));
        assert_eq!(report.synced_files, 1);
    }

    #[tokio::test]
    async fn provider_path_scope_no_escape_rejects_traversal_like_inputs() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let outside_root = tempdir.path().join("outside");
        let memory_root = tempdir.path().join("memories");
        tokio::fs::create_dir_all(&memory_root)
            .await
            .expect("create memory root");
        tokio::fs::create_dir_all(&outside_root)
            .await
            .expect("create outside dir");
        tokio::fs::write(outside_root.join("outside.md"), "outside\n")
            .await
            .expect("write outside file");
        let backend = LocalMemoriesBackend::from_memory_root(&memory_root);

        let backend_path = "../outside.md";
        assert!(matches!(
            MemoriesBackend::read(
                &backend,
                ReadMemoryRequest {
                    path: backend_path.to_string(),
                    line_offset: 1,
                    max_lines: Some(1),
                    max_tokens: 1024,
                },
            )
            .await
            .expect_err("traversal read should be rejected"),
            MemoriesBackendError::InvalidPath { path, reason }
                if path == backend_path && reason == "must stay within the memories root"
        ));
        assert!(matches!(
            MemoriesBackend::search(
                &backend,
                SearchMemoriesRequest {
                    queries: vec!["outside".to_string()],
                    match_mode: SearchMatchMode::Any,
                    path: Some(backend_path.to_string()),
                    cursor: None,
                    context_lines: 0,
                    case_sensitive: false,
                    normalized: false,
                    max_results: 10,
                },
            )
            .await
            .expect_err("traversal search should be rejected"),
            MemoriesBackendError::InvalidPath { path, reason }
                if path == backend_path && reason == "must stay within the memories root"
        ));
        assert!(matches!(
            MemoriesBackend::list(
                &backend,
                ListMemoriesRequest {
                    path: Some(backend_path.to_string()),
                    cursor: None,
                    max_results: 10,
                },
            )
            .await
            .expect_err("traversal list should be rejected"),
            MemoriesBackendError::InvalidPath { path, reason }
                if path == backend_path && reason == "must stay within the memories root"
        ));
    }
}
