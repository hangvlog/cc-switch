use std::path::PathBuf;

use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::clawkit_account::transport::{
    http_client_builder, post_json_with_bearer_direct_fallback,
};

const DEFAULT_GATEWAY_API_BASE: &str = "https://api.clawkit.chat";

#[derive(Debug, Clone)]
pub struct GatewayBootstrap {
    pub api_key: String,
    pub base_url: String,
    pub models: Vec<String>,
    pub available_quota: i64,
    pub used_quota: i64,
}

#[derive(Debug, Deserialize)]
struct BootstrapEnvelope {
    success: bool,
    #[serde(default)]
    message: String,
    data: Option<BootstrapData>,
}

#[derive(Debug, Deserialize)]
struct BootstrapData {
    api_key: String,
    base_url: String,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    quota: BootstrapQuota,
}

#[derive(Debug, Default, Deserialize)]
struct BootstrapQuota {
    #[serde(default)]
    available: i64,
    #[serde(default)]
    used: i64,
}

pub async fn bootstrap() -> Result<GatewayBootstrap, String> {
    let api_base = std::env::var("CLAWKIT_GATEWAY_API_BASE_URL")
        .unwrap_or_else(|_| DEFAULT_GATEWAY_API_BASE.to_string());
    let api_base = normalize_api_base(&api_base)?;
    let (account_token, device_id) =
        crate::clawkit_account::ClawkitAccountClient::default().active_credentials()?;
    let client = http_client_builder()
        .build()
        .map_err(|error| error.to_string())?;
    let direct_client = http_client_builder()
        .no_proxy()
        .build()
        .map_err(|error| error.to_string())?;
    let endpoint = format!("{api_base}/api/user/clawkit/codex/bootstrap");
    let response = post_json_with_bearer_direct_fallback(
        &client,
        &direct_client,
        &endpoint,
        &account_token,
        &json!({ "device_id": device_id }),
    )
    .await?;
    let status = response.status();
    let envelope = response
        .json::<BootstrapEnvelope>()
        .await
        .map_err(|_| "ClawKit API 代理响应无效".to_string())?;
    if status == StatusCode::UNAUTHORIZED {
        return Err("ClawKit 登录已过期，请重新登录".to_string());
    }
    if !status.is_success() || !envelope.success {
        return Err(if envelope.message.is_empty() {
            format!("ClawKit API 代理初始化失败 ({})", status.as_u16())
        } else {
            envelope.message
        });
    }
    let data = envelope
        .data
        .ok_or_else(|| "ClawKit API 代理响应缺少配置".to_string())?;
    if data.api_key.trim().is_empty() || data.models.is_empty() {
        return Err("当前账号没有可用的 API 模型".to_string());
    }
    Ok(GatewayBootstrap {
        api_key: data.api_key,
        base_url: normalize_api_base(&data.base_url)?,
        models: data.models,
        available_quota: data.quota.available,
        used_quota: data.quota.used,
    })
}

pub(crate) fn write_model_catalog_at(models: &[String], path: PathBuf) -> Result<PathBuf, String> {
    let mut unique = normalized_models(models);
    unique.sort();
    if unique.is_empty() {
        return Err("当前账号没有可用的 API 模型".to_string());
    }
    let template: Value = serde_json::from_str(include_str!(
        "resources/codex_native_responses_template.json"
    ))
    .map_err(|error| error.to_string())?;
    let entries = unique
        .into_iter()
        .enumerate()
        .map(|(priority, slug)| {
            let mut entry = template.clone();
            if let Some(object) = entry.as_object_mut() {
                object.insert("slug".into(), json!(slug));
                object.insert("display_name".into(), json!(slug));
                object.insert("description".into(), json!(slug));
                object.insert("priority".into(), json!(priority));
            }
            entry
        })
        .collect::<Vec<_>>();
    let contents = serde_json::to_vec_pretty(&json!({ "models": entries }))
        .map_err(|error| error.to_string())?;
    crate::config::atomic_write(&path, &contents).map_err(|error| error.to_string())?;
    Ok(path)
}

pub fn normalized_models(models: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    for model in models.iter().map(|model| model.trim()) {
        if !model.is_empty() && !normalized.iter().any(|current| current == model) {
            normalized.push(model.to_string());
        }
    }
    normalized
}

pub fn preferred_default_model(models: &[String]) -> Option<&str> {
    const PREFERRED: &[&str] = &[
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
        "gpt-5.6",
        "gpt-5.5",
        "gpt-5.4",
        "gpt-5.2",
    ];
    PREFERRED
        .iter()
        .find_map(|preferred| models.iter().find(|model| model.as_str() == *preferred))
        .or_else(|| models.first())
        .map(String::as_str)
}

fn normalize_api_base(value: &str) -> Result<String, String> {
    let value = value.trim().trim_end_matches('/');
    if !(value.starts_with("https://") || value.starts_with("http://")) {
        return Err("ClawKit API 地址必须使用 HTTP 或 HTTPS".to_string());
    }
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::{preferred_default_model, write_model_catalog_at};

    #[test]
    fn sol_is_the_preferred_account_model() {
        let models = vec!["gpt-5.6-terra".into(), "gpt-5.6-sol".into()];
        assert_eq!(preferred_default_model(&models), Some("gpt-5.6-sol"));
    }

    #[test]
    fn catalog_contains_every_account_model_without_secrets() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = write_model_catalog_at(
            &["gpt-5.6-sol".into(), "gpt-5.6-terra".into()],
            temp.path().join("clawkit-models.json"),
        )
        .expect("catalog");
        let text = std::fs::read_to_string(path).expect("read catalog");
        assert!(text.contains("gpt-5.6-sol"));
        assert!(text.contains("gpt-5.6-terra"));
        assert!(!text.contains("api_key"));
    }
}
