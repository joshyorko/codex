use super::*;
use pretty_assertions::assert_eq;

#[test]
fn deserialize_skill_config_with_name_selector() {
    let cfg: SkillConfig = toml::from_str(
        r#"
            name = "github:yeet"
            enabled = false
        "#,
    )
    .expect("should deserialize skill config with name selector");

    assert_eq!(cfg.name.as_deref(), Some("github:yeet"));
    assert_eq!(cfg.path, None);
    assert!(!cfg.enabled);
}

#[test]
fn deserialize_skill_config_with_path_selector() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let skill_path = tempdir.path().join("skills").join("demo").join("SKILL.md");
    let cfg: SkillConfig = toml::from_str(&format!(
        r#"
            path = {path:?}
            enabled = false
        "#,
        path = skill_path.display().to_string(),
    ))
    .expect("should deserialize skill config with path selector");

    assert_eq!(
        cfg,
        SkillConfig {
            path: Some(
                AbsolutePathBuf::from_absolute_path(&skill_path)
                    .expect("skill path should be absolute"),
            ),
            name: None,
            enabled: false,
        }
    );
}

#[test]
fn memories_config_clamps_count_limits_to_nonzero_values() {
    let config = MemoriesConfig::from(MemoriesToml {
        max_raw_memories_for_consolidation: Some(0),
        max_rollouts_per_startup: Some(0),
        ..Default::default()
    });

    assert_eq!(
        config,
        MemoriesConfig {
            max_raw_memories_for_consolidation: 1,
            max_rollouts_per_startup: 1,
            ..MemoriesConfig::default()
        }
    );
}

#[test]
fn memories_config_clamps_rate_limit_remaining_threshold() {
    let config = MemoriesConfig::from(MemoriesToml {
        min_rate_limit_remaining_percent: Some(101),
        ..Default::default()
    });
    assert_eq!(
        config,
        MemoriesConfig {
            min_rate_limit_remaining_percent: 100,
            ..MemoriesConfig::default()
        }
    );

    let config = MemoriesConfig::from(MemoriesToml {
        min_rate_limit_remaining_percent: Some(-1),
        ..Default::default()
    });
    assert_eq!(
        config,
        MemoriesConfig {
            min_rate_limit_remaining_percent: 0,
            ..MemoriesConfig::default()
        }
    );
}

#[test]
fn memories_config_defaults_to_local_backend() {
    let config = MemoriesConfig::from(MemoriesToml::default());

    assert_eq!(
        config,
        MemoriesConfig {
            backend: MemoryBackendKind::Local,
            profile: MemoryProfile::Personal,
            workspace: "default".to_string(),
            user_peer: "user".to_string(),
            assistant_peer: "codex".to_string(),
            honcho_base_url: None,
            honcho_api_key_env: Some("HONCHO_API_KEY".to_string()),
            write_policy: MemoryWritePolicy::VisibleTurns,
            sync_policy: MemorySyncPolicy::Manual,
            cross_profile_policy: CrossProfilePolicy::DefaultDeny,
            ..MemoriesConfig::default()
        }
    );
}

#[test]
fn memories_config_parses_honcho_backend_compatibility_fields() {
    let toml: MemoriesToml = toml::from_str(
        r#"
            backend = "honcho"
            profile = "work"
            workspace = "codex-memory-lab"
            user_peer = "josh"
            assistant_peer = "codex"
            honcho_base_url = "http://localhost:8000/v3"
            honcho_api_key_env = "HONCHO_DEV_KEY"
            write_policy = "visible_turns"
            sync_policy = "startup"
            cross_profile_policy = "default_deny"
        "#,
    )
    .expect("memories TOML should parse");

    assert_eq!(
        MemoriesConfig::from(toml),
        MemoriesConfig {
            backend: MemoryBackendKind::Provider,
            provider: MemoryProviderKind::Honcho,
            profile: MemoryProfile::Work,
            workspace: "codex-memory-lab".to_string(),
            user_peer: "josh".to_string(),
            assistant_peer: "codex".to_string(),
            provider_url: Some("http://localhost:8000/v3".to_string()),
            honcho_base_url: Some("http://localhost:8000/v3".to_string()),
            honcho_api_key_env: Some("HONCHO_DEV_KEY".to_string()),
            write_policy: MemoryWritePolicy::VisibleTurns,
            sync_policy: MemorySyncPolicy::Startup,
            local_import_policy: LocalImportPolicy::StartupApply,
            cross_profile_policy: CrossProfilePolicy::DefaultDeny,
            ..MemoriesConfig::default()
        }
    );
}

#[test]
fn memories_config_parses_codex_memoryd_provider_contract() {
    let toml: MemoriesToml = toml::from_str(
        r#"
            backend = "provider"
            provider = "codex_memoryd"
            provider_url = "http://127.0.0.1:8787"
            local_import_policy = "startup_preview"
        "#,
    )
    .expect("memories TOML should parse");

    assert_eq!(
        MemoriesConfig::from(toml),
        MemoriesConfig {
            backend: MemoryBackendKind::Provider,
            provider: MemoryProviderKind::CodexMemoryd,
            provider_url: Some("http://127.0.0.1:8787".to_string()),
            local_import_policy: LocalImportPolicy::StartupPreview,
            ..MemoriesConfig::default()
        }
    );
}
