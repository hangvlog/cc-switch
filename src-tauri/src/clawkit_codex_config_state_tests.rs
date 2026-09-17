use super::{apply, rollback, PROVIDER_ID};
use crate::app_config::AppType;
use crate::clawkit_gateway::GatewayBootstrap;
use crate::database::Database;
use crate::provider::Provider;
use crate::store::AppState;
use serde_json::json;
use serial_test::serial;
use std::sync::Arc;

struct TestHome {
    _dir: tempfile::TempDir,
    previous_test_home: Option<std::ffi::OsString>,
}

impl TestHome {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let previous_test_home = std::env::var_os("CC_SWITCH_TEST_HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", dir.path());
        crate::settings::reload_settings().expect("reload isolated settings");
        crate::settings::update_settings(crate::settings::AppSettings::default())
            .expect("reset isolated settings");
        Self {
            _dir: dir,
            previous_test_home,
        }
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        match self.previous_test_home.take() {
            Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
        let _ = crate::settings::reload_settings();
    }
}

const CUSTOM_CONFIG: &str = r#"model_provider = "custom"
model = "gpt-old"

[model_providers.custom]
name = "custom"
base_url = "https://old.example/v1"
wire_api = "responses"
requires_openai_auth = false
experimental_bearer_token = "old-key"

[desktop]
followUpQueueMode = "steer"
"#;

async fn seed_custom_takeover(state: &AppState) -> Provider {
    let custom = Provider::with_id(
        "custom".to_string(),
        "Custom".to_string(),
        json!({ "auth": {}, "config": CUSTOM_CONFIG }),
        None,
    );
    state
        .db
        .save_provider(AppType::Codex.as_str(), &custom)
        .expect("seed custom provider");
    state
        .db
        .set_current_provider(AppType::Codex.as_str(), &custom.id)
        .expect("seed DB current");
    crate::settings::set_current_provider(&AppType::Codex, Some(&custom.id))
        .expect("seed local current");
    state
        .db
        .save_live_backup(
            AppType::Codex.as_str(),
            &serde_json::to_string(&custom.settings_config).expect("serialize takeover backup"),
        )
        .await
        .expect("seed takeover backup");
    let mut takeover = state
        .db
        .get_proxy_config_for_app(AppType::Codex.as_str())
        .await
        .expect("read Codex proxy config");
    takeover.enabled = true;
    state
        .db
        .update_proxy_config_for_app(takeover)
        .await
        .expect("seed enabled takeover");
    std::fs::write(
        crate::codex_config::get_codex_config_dir().join("config.toml"),
        r#"model_provider = "custom"
[model_providers.custom]
base_url = "http://127.0.0.1:15721/v1"
experimental_bearer_token = "PROXY_MANAGED"
"#,
    )
    .expect("seed takeover live config");
    custom
}

#[tokio::test]
#[serial]
async fn one_click_setup_replaces_custom_current_and_survives_takeover_recovery() {
    let _home = TestHome::new();
    let codex_home = crate::codex_config::get_codex_config_dir();
    std::fs::create_dir_all(&codex_home).expect("create Codex home");

    let db = Arc::new(Database::memory().expect("in-memory database"));
    let state = AppState::new(db.clone());
    seed_custom_takeover(&state).await;

    let gateway = GatewayBootstrap {
        api_key: "sk-private".into(),
        models: vec!["gpt-5.6-sol".into()],
        available_quota: 100,
        used_quota: 5,
    };
    let configured = apply(&gateway, &state, None, Some("https://api.clawkit.chat/v1"))
        .await
        .expect("one-click setup");
    assert!(configured.configured);

    let live = std::fs::read_to_string(codex_home.join("config.toml"))
        .expect("read configured Live config");
    assert!(live.contains(r#"model_provider = "clawkit""#), "{live}");
    assert!(
        live.contains(r#"base_url = "https://api.clawkit.chat/v1""#),
        "{live}"
    );
    assert!(live.contains("[desktop]"), "{live}");
    assert_eq!(
        crate::settings::get_current_provider(&AppType::Codex).as_deref(),
        Some(PROVIDER_ID)
    );
    assert_eq!(
        db.get_current_provider(AppType::Codex.as_str())
            .expect("read DB current")
            .as_deref(),
        Some(PROVIDER_ID)
    );
    let stored = db
        .get_provider_by_id(PROVIDER_ID, AppType::Codex.as_str())
        .expect("read ClawKit provider")
        .expect("ClawKit provider must be registered");
    assert_eq!(
        stored.meta.and_then(|meta| meta.api_format).as_deref(),
        Some("openai_responses")
    );
    assert!(
        !db.get_proxy_config_for_app(AppType::Codex.as_str())
            .await
            .expect("read takeover state")
            .enabled
    );
    assert!(db
        .get_live_backup(AppType::Codex.as_str())
        .await
        .expect("read takeover backup")
        .is_none());

    state
        .proxy_service
        .recover_from_crash()
        .await
        .expect("simulate restart recovery");
    let after_recovery = std::fs::read_to_string(codex_home.join("config.toml"))
        .expect("read Live config after recovery");
    assert!(
        after_recovery.contains(r#"model_provider = "clawkit""#),
        "{after_recovery}"
    );

    let rolled_back = rollback(&state).await.expect("rollback one-click setup");
    assert!(!rolled_back.configured);
    assert_eq!(
        crate::settings::get_current_provider(&AppType::Codex).as_deref(),
        Some("custom")
    );
    assert_eq!(
        db.get_current_provider(AppType::Codex.as_str())
            .expect("read restored DB current")
            .as_deref(),
        Some("custom")
    );
    assert!(db
        .get_provider_by_id(PROVIDER_ID, AppType::Codex.as_str())
        .expect("read restored ClawKit provider")
        .is_none());
    assert!(
        db.get_proxy_config_for_app(AppType::Codex.as_str())
            .await
            .expect("read restored takeover state")
            .enabled
    );
    assert!(db
        .get_live_backup(AppType::Codex.as_str())
        .await
        .expect("read restored takeover backup")
        .is_some());
    let live_after_rollback = std::fs::read_to_string(codex_home.join("config.toml"))
        .expect("read Live config after rollback");
    assert!(
        live_after_rollback.contains("http://127.0.0.1:15721/v1"),
        "{live_after_rollback}"
    );
}

#[tokio::test]
#[serial]
async fn failed_one_click_setup_restores_previous_takeover_and_current_provider() {
    let _home = TestHome::new();
    let codex_home = crate::codex_config::get_codex_config_dir();
    std::fs::create_dir_all(&codex_home).expect("create Codex home");
    let db = Arc::new(Database::memory().expect("in-memory database"));
    let state = AppState::new(db.clone());
    seed_custom_takeover(&state).await;

    let gateway = GatewayBootstrap {
        api_key: "sk-private".into(),
        models: Vec::new(),
        available_quota: 100,
        used_quota: 5,
    };
    let error = apply(&gateway, &state, None, Some("https://api.clawkit.chat/v1"))
        .await
        .expect_err("empty model list must fail");
    assert!(error.contains("没有可用的 API 模型"), "{error}");
    assert_eq!(
        crate::settings::get_current_provider(&AppType::Codex).as_deref(),
        Some("custom")
    );
    assert_eq!(
        db.get_current_provider(AppType::Codex.as_str())
            .expect("read restored DB current")
            .as_deref(),
        Some("custom")
    );
    assert!(
        db.get_proxy_config_for_app(AppType::Codex.as_str())
            .await
            .expect("read restored takeover state")
            .enabled
    );
    assert!(db
        .get_live_backup(AppType::Codex.as_str())
        .await
        .expect("read restored takeover backup")
        .is_some());
    let live = std::fs::read_to_string(codex_home.join("config.toml"))
        .expect("read Live config after failed one-click setup");
    assert!(live.contains("http://127.0.0.1:15721/v1"), "{live}");
}

#[tokio::test]
#[serial]
async fn file_only_setup_is_missing_from_list_until_provider_sync_and_reconfigure_updates_it() {
    let _home = TestHome::new();
    let codex_home = crate::codex_config::get_codex_config_dir();
    let db = Arc::new(Database::memory().expect("in-memory database"));
    let state = AppState::new(db.clone());
    let gateway = GatewayBootstrap {
        api_key: "sk-test-only".into(),
        models: vec!["model-a".into(), "model-b".into()],
        available_quota: 100,
        used_quota: 0,
    };
    // Reproduce the old command: writing files alone leaves the list empty.
    super::apply_at_home(&gateway, &codex_home, Some("model-a"), None).unwrap();
    assert!(db.get_all_providers("codex").unwrap().is_empty());
    apply(&gateway, &state, Some("model-a"), None)
        .await
        .unwrap();
    let providers = db.get_all_providers("codex").unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[PROVIDER_ID].name, "ClawKit API");
    assert!(providers[PROVIDER_ID].settings_config["config"]
        .as_str()
        .unwrap()
        .contains(super::DEFAULT_CODEX_API_BASE_URL));
    apply(
        &gateway,
        &state,
        Some("model-b"),
        Some("https://custom.example/v1"),
    )
    .await
    .unwrap();
    let providers = db.get_all_providers("codex").unwrap();
    assert_eq!(
        providers.len(),
        1,
        "reconfiguration must update the same entry"
    );
    let live = std::fs::read_to_string(codex_home.join("config.toml")).unwrap();
    assert_eq!(providers[PROVIDER_ID].settings_config["config"], live);
    assert!(live.contains("model-b"));
    assert!(live.contains("https://custom.example/v1"));
    assert_eq!(
        db.get_current_provider("codex").unwrap().as_deref(),
        Some(PROVIDER_ID)
    );
}
