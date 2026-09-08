use super::{
    apply_at_home, managed_config_text, resolve_model, rollback_at_home, status_from_home,
    MODEL_CATALOG_FILE, PROVIDER_ID,
};
use crate::clawkit_gateway::GatewayBootstrap;
use toml_edit::{DocumentMut, Item};

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
    assert!(!updated.contains("web_search"));
    assert!(updated.contains("base_url = \"https://api.clawkit.chat/v1\""));
    assert!(updated.contains("experimental_bearer_token = \"sk-private\""));
}

#[test]
fn configuration_preserves_the_users_web_search_preference() {
    let updated = managed_config_text(
        "web_search = \"live\"\n",
        "https://api.clawkit.chat/v1",
        "sk-private",
        "gpt-5.6-sol",
    )
    .unwrap();
    let doc = updated.parse::<DocumentMut>().unwrap();
    assert_eq!(
        doc.get(crate::codex_config::CODEX_WEB_SEARCH_FIELD)
            .and_then(Item::as_str),
        Some("live")
    );
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

    let applied = apply_at_home(&gateway, temp.path(), None).unwrap();
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

#[test]
fn apply_uses_an_allowed_selected_model() {
    let temp = tempfile::tempdir().unwrap();
    let gateway = GatewayBootstrap {
        api_key: "sk-private".into(),
        base_url: "https://gateway.test/v1".into(),
        models: vec!["gpt-5.6-sol".into(), "gpt-5.6-terra".into()],
        available_quota: 100,
        used_quota: 5,
    };

    let applied = apply_at_home(&gateway, temp.path(), Some("gpt-5.6-terra")).unwrap();
    assert_eq!(applied.model.as_deref(), Some("gpt-5.6-terra"));
    let configured = std::fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(configured.contains("model = \"gpt-5.6-terra\""));
}

#[test]
fn apply_rejects_a_model_outside_the_account_allowlist() {
    let temp = tempfile::tempdir().unwrap();
    let gateway = GatewayBootstrap {
        api_key: "sk-private".into(),
        base_url: "https://gateway.test/v1".into(),
        models: vec!["gpt-5.6-sol".into()],
        available_quota: 100,
        used_quota: 5,
    };

    let error = apply_at_home(&gateway, temp.path(), Some("unauthorized-model")).unwrap_err();
    assert!(error.contains("不在当前账号"));
    assert!(!temp.path().join("config.toml").exists());
}

#[test]
fn empty_selection_keeps_the_preferred_sol_default() {
    let models = vec!["gpt-5.6-terra".into(), "gpt-5.6-sol".into()];
    assert_eq!(resolve_model(&models, None).unwrap(), "gpt-5.6-sol");
}
