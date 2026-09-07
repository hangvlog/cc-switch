use super::{
    apply, apply_at_home, managed_config_text, rollback, rollback_at_home, status_from_home,
    MODEL_CATALOG_FILE, PROVIDER_ID,
};
use crate::app_config::AppType;
use crate::clawkit_gateway::GatewayBootstrap;
use crate::database::Database;
use crate::provider::Provider;
use crate::store::AppState;
use serde_json::json;
use serial_test::serial;
use std::sync::Arc;
use toml_edit::{DocumentMut, Item};

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

#[test]
fn configuration_preserves_unrelated_codex_settings() {
    let existing = r#"approval_policy = "on-request"

[mcp_servers.keep_me]
command = "example"
"#;
    let updated = managed_config_text(
        existing,
        "https://api.clawkit.chat/v1/",
        "sk-private",
        "gpt-5.6-sol",
    )
    .unwrap();
    assert!(updated.contains("approval_policy = \"on-request\""));
    assert!(updated.contains("[mcp_servers.keep_me]"));
    assert!(updated.contains("model_provider = \"clawkit\""));
    assert!(updated.contains("base_url = \"https://api.clawkit.chat/v1\""));
    assert!(updated.contains("experimental_bearer_token = \"sk-private\""));
}

#[test]
fn status_requires_managed_config_and_catalog() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("config.toml"),
        managed_config_text("", "https://gateway.test/v1", "secret", "model-a").unwrap(),
    )
    .unwrap();
    assert!(!status_from_home(temp.path()).configured);
    std::fs::write(
        temp.path().join("clawkit-models.json"),
        r#"{"models":[{"slug":"model-a"}]}"#,
    )
    .unwrap();
    let status = status_from_home(temp.path());
    assert!(status.configured);
    assert_eq!(status.model.as_deref(), Some("model-a"));
    assert_eq!(status.models, ["model-a"]);
}

#[test]
fn apply_preserves_auth_and_unrelated_config_then_rolls_back() {
    let temp = tempfile::tempdir().unwrap();
    let auth_path = temp.path().join("auth.json");
    let config_path = temp.path().join("config.toml");
    std::fs::write(&auth_path, br#"{"tokens":{"access_token":"official"}}"#).unwrap();
    std::fs::write(
        &config_path,
        "approval_policy = \"on-request\"\n[mcp_servers.keep_me]\ncommand = \"example\"\n",
    )
    .unwrap();
    let original_auth = std::fs::read(&auth_path).unwrap();
    let original_config = std::fs::read(&config_path).unwrap();
    let gateway = GatewayBootstrap {
        api_key: "sk-private".into(),
        base_url: "https://gateway.test/v1".into(),
        models: vec!["gpt-5.6-sol".into()],
        available_quota: 100,
        used_quota: 5,
    };

    let applied = apply_at_home(&gateway, temp.path()).unwrap();
    assert!(applied.configured);
    assert!(applied.can_rollback);
    assert_eq!(std::fs::read(&auth_path).unwrap(), original_auth);
    let configured = std::fs::read_to_string(&config_path).unwrap();
    assert!(configured.contains("[mcp_servers.keep_me]"));
    let configured_doc = configured.parse::<DocumentMut>().unwrap();
    let configured_token = configured_doc
        .get("model_providers")
        .and_then(Item::as_table)
        .and_then(|providers| providers.get(PROVIDER_ID))
        .and_then(Item::as_table)
        .and_then(|provider| provider.get("experimental_bearer_token"))
        .and_then(Item::as_str);
    assert_eq!(configured_token, Some("sk-private"), "{configured}");

    let restored = rollback_at_home(temp.path()).unwrap();
    assert!(!restored.configured);
    assert_eq!(std::fs::read(&auth_path).unwrap(), original_auth);
    assert_eq!(std::fs::read(&config_path).unwrap(), original_config);
    assert!(!temp.path().join(MODEL_CATALOG_FILE).exists());
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
        base_url: "https://api.clawkit.chat/v1".into(),
        models: vec!["gpt-5.6-sol".into()],
        available_quota: 100,
        used_quota: 5,
    };
    let configured = apply(&gateway, &state).await.expect("one-click setup");
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
        base_url: "https://api.clawkit.chat/v1".into(),
        models: Vec::new(),
        available_quota: 100,
        used_quota: 5,
    };
    let error = apply(&gateway, &state)
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
