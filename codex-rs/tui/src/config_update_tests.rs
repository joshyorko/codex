use super::*;
use codex_config::types::MemoryBackendKind;
use codex_config::types::MemoryProfile;
use codex_config::types::MemoryProviderKind;
use codex_config::types::MemoryWritePolicy;
use color_eyre::eyre::WrapErr;
use pretty_assertions::assert_eq;
use std::path::Path;

#[test]
fn app_scoped_key_path_quotes_dotted_app_ids() {
    assert_eq!(
        app_scoped_key_path("plugin.linear", "enabled"),
        "apps.\"plugin.linear\".enabled"
    );
}

#[test]
fn trusted_project_edit_targets_project_trust_level() {
    assert_eq!(
        trusted_project_edit(Path::new("/workspace/team.project")),
        ConfigEdit {
            key_path: "projects.\"/workspace/team.project\".trust_level".to_string(),
            value: serde_json::json!("trusted"),
            merge_strategy: MergeStrategy::Replace,
        }
    );
}

#[test]
fn format_config_error_preserves_server_validation_message() {
    let err = Err::<(), _>(color_eyre::eyre::eyre!(
        "config/batchWrite failed: Invalid configuration: features.fast_mode=true violates \
         managed requirements; allowed set [fast_mode=false]"
    ))
    .wrap_err("config/batchWrite failed in TUI")
    .unwrap_err();

    assert_eq!(
        format_config_error(&err),
        "config/batchWrite failed in TUI: config/batchWrite failed: Invalid configuration: \
         features.fast_mode=true violates managed requirements; allowed set [fast_mode=false]"
    );
}

#[test]
fn portable_memory_setup_edits_configure_codex_memoryd_without_secrets() {
    let setup = PortableMemorySetup {
        backend: MemoryBackendKind::Hybrid,
        provider: MemoryProviderKind::CodexMemoryd,
        profile: MemoryProfile::Oss,
        workspace: "codex-memory-lab".to_string(),
        user_peer: "josh".to_string(),
        assistant_peer: "codex".to_string(),
        provider_url: Some("http://127.0.0.1:8787".to_string()),
        honcho_api_key_env: None,
    };

    let edits = build_portable_memory_setup_edits(&setup);

    assert!(edits.contains(&replace_config_value(
        "memories.backend",
        serde_json::json!("hybrid")
    )));
    assert!(edits.contains(&replace_config_value(
        "memories.provider",
        serde_json::json!("codex_memoryd")
    )));
    assert!(edits.contains(&replace_config_value(
        "memories.profile",
        serde_json::json!("oss")
    )));
    assert!(edits.contains(&replace_config_value(
        "memories.provider_url",
        serde_json::json!("http://127.0.0.1:8787")
    )));
    assert!(edits.contains(&replace_config_value(
        "memories.write_policy",
        serde_json::json!(MemoryWritePolicy::VisibleTurns.as_str())
    )));
    assert!(edits.contains(&clear_config_value("memories.honcho_api_key_env")));
}

#[test]
fn portable_memory_setup_edits_configure_honcho_env_var_not_secret() {
    let setup = PortableMemorySetup {
        backend: MemoryBackendKind::Provider,
        provider: MemoryProviderKind::Honcho,
        profile: MemoryProfile::Personal,
        workspace: "default".to_string(),
        user_peer: "user".to_string(),
        assistant_peer: "codex".to_string(),
        provider_url: None,
        honcho_api_key_env: Some("HONCHO_TOKEN".to_string()),
    };

    let edits = build_portable_memory_setup_edits(&setup);

    assert!(edits.contains(&replace_config_value(
        "memories.provider",
        serde_json::json!("honcho")
    )));
    assert!(edits.contains(&replace_config_value(
        "memories.honcho_api_key_env",
        serde_json::json!("HONCHO_TOKEN")
    )));
    assert!(edits.contains(&clear_config_value("memories.provider_url")));
    assert!(!edits.iter().any(|edit| edit.key_path.contains("api_key")
        && edit.value == serde_json::json!("HONCHO_SECRET_VALUE")));
}

#[test]
fn portable_memory_disable_only_switches_backend_to_local() {
    assert_eq!(
        build_portable_memory_disable_edits(),
        vec![replace_config_value(
            "memories.backend",
            serde_json::json!("local")
        )]
    );
}
