use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

mod transport;

use transport::{http_client_builder, post_json_with_direct_fallback};

const DEFAULT_ACCOUNT_API_BASE: &str = "https://image.clawkit.chat";
const DEFAULT_RELAY_API_BASE: &str = "https://clawkit.chat";
const SESSION_FILE: &str = "clawkit-account.json";
const PRODUCT: &str = "codex-remote";
const DEVICE_NAME: &str = "ClawKit Desktop";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredSession {
    token: String,
    user: Value,
    device_id: String,
    expires_at: u64,
}

#[derive(Clone)]
pub struct ClawkitAccountClient {
    account_api_base: String,
    relay_api_base: String,
    session_path: PathBuf,
    client: reqwest::Client,
    direct_client: reqwest::Client,
}

impl Default for ClawkitAccountClient {
    fn default() -> Self {
        let legacy_base = std::env::var("CLAWKIT_API_BASE_URL").ok();
        let account_api_base = std::env::var("CLAWKIT_ACCOUNT_API_BASE_URL")
            .ok()
            .or_else(|| legacy_base.clone())
            .unwrap_or_else(|| DEFAULT_ACCOUNT_API_BASE.to_string());
        let relay_api_base = std::env::var("CLAWKIT_RELAY_API_BASE_URL")
            .ok()
            .or(legacy_base)
            .unwrap_or_else(|| DEFAULT_RELAY_API_BASE.to_string());
        Self::with_endpoints(account_api_base, relay_api_base, default_session_path())
            .expect("ClawKit account client should initialize")
    }
}

impl ClawkitAccountClient {
    pub fn with_endpoints(
        account_api_base: impl Into<String>,
        relay_api_base: impl Into<String>,
        session_path: impl Into<PathBuf>,
    ) -> Result<Self, String> {
        let account_api_base = normalize_api_base(&account_api_base.into())?;
        let relay_api_base = normalize_api_base(&relay_api_base.into())?;
        let client = http_client_builder()
            .build()
            .map_err(|error| error.to_string())?;
        let direct_client = http_client_builder()
            .no_proxy()
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            account_api_base,
            relay_api_base,
            session_path: session_path.into(),
            client,
            direct_client,
        })
    }

    pub fn status(&self) -> Value {
        match self.load_active_session() {
            Ok(session) => session_status(&session),
            Err(_) => json!({ "status": "ok", "authenticated": false }),
        }
    }

    pub async fn login(&self, username: &str, password: &str) -> Result<Value, String> {
        let username = username.trim();
        if username.is_empty() || password.is_empty() {
            return Err("请输入账号和密码".to_string());
        }
        let device_id = self
            .load_session()
            .ok()
            .map(|session| session.device_id)
            .unwrap_or_else(|| format!("clawkit-desktop-{}", Uuid::new_v4()));
        let (endpoint, payload) =
            login_request(&self.account_api_base, username, password, &device_id);
        let response =
            post_json_with_direct_fallback(&self.client, &self.direct_client, &endpoint, &payload)
                .await?;
        let status = response.status();
        let body = response.json::<Value>().await.unwrap_or(Value::Null);
        if !status.is_success() || body.get("code").and_then(Value::as_i64) != Some(200) {
            return Err(response_message(&body, status, "登录失败"));
        }
        let data = body.get("data").cloned().unwrap_or(Value::Null);
        let token = data
            .get("token")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "登录响应缺少账号令牌".to_string())?;
        let expires_in = data
            .get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(24 * 60 * 60);
        let session = StoredSession {
            token: token.to_string(),
            user: data
                .get("user")
                .cloned()
                .unwrap_or_else(|| json!({ "username": username })),
            device_id,
            expires_at: unix_timestamp().saturating_add(expires_in),
        };
        self.save_session(&session)?;
        Ok(session_status(&session))
    }

    pub fn logout(&self) -> Result<Value, String> {
        if self.session_path.exists() {
            fs::remove_file(&self.session_path)
                .map_err(|error| format!("无法清除 ClawKit 登录状态: {error}"))?;
        }
        Ok(json!({ "status": "ok", "authenticated": false }))
    }

    pub async fn create_socket_ticket(&self) -> Result<Value, String> {
        let session = self.load_active_session()?;
        let response = self
            .client
            .post(format!(
                "{}/api/codex-remote/account/socket-ticket",
                self.relay_api_base
            ))
            .bearer_auth(&session.token)
            .json(&json!({
                "role": "desktop",
                "device_id": session.device_id,
                "device_name": DEVICE_NAME,
            }))
            .send()
            .await
            .map_err(|_| "无法连接 ClawKit 远程服务".to_string())?;
        let status = response.status();
        let body = response.json::<Value>().await.unwrap_or(Value::Null);
        if status == StatusCode::UNAUTHORIZED {
            let _ = self.logout();
            return Err("登录已过期，请重新登录".to_string());
        }
        let ticket = body
            .get("ticket")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !status.is_success() || ticket.is_empty() {
            return Err(response_message(&body, status, "创建安全连接失败"));
        }
        Ok(json!({
            "status": "ok",
            "websocket_url": format!(
                "{}/api/codex-remote/account/ws?ticket={}",
                websocket_base(&self.relay_api_base),
                ticket
            ),
            "expires_at": body.get("expires_at").cloned().unwrap_or(Value::Null),
            "device_id": session.device_id,
        }))
    }

    pub fn active_credentials(&self) -> Result<(String, String), String> {
        let session = self.load_active_session()?;
        Ok((session.token, session.device_id))
    }

    fn load_active_session(&self) -> Result<StoredSession, String> {
        let session = self.load_session()?;
        if session.expires_at <= unix_timestamp() {
            let _ = self.logout();
            return Err("登录已过期，请重新登录".to_string());
        }
        Ok(session)
    }

    fn load_session(&self) -> Result<StoredSession, String> {
        let text =
            fs::read_to_string(&self.session_path).map_err(|_| "尚未登录 ClawKit".to_string())?;
        serde_json::from_str(&text).map_err(|_| "ClawKit 登录状态无效".to_string())
    }

    fn save_session(&self, session: &StoredSession) -> Result<(), String> {
        let text = serde_json::to_string_pretty(session).map_err(|error| error.to_string())?;
        crate::config::atomic_write(&self.session_path, text.as_bytes())
            .map_err(|error| error.to_string())?;
        restrict_session_permissions(&self.session_path)
    }
}

fn login_request(api_base: &str, login: &str, password: &str, device_id: &str) -> (String, Value) {
    if is_mainland_phone(login) {
        return (
            format!("{api_base}/api/auth/phone/login"),
            json!({
                "phone": login,
                "password": password,
                "device_id": device_id,
                "product": PRODUCT,
            }),
        );
    }
    (
        format!("{api_base}/auth/login"),
        json!({
            "username": login,
            "password": password,
            "device_id": device_id,
            "product": PRODUCT,
        }),
    )
}

fn is_mainland_phone(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 11
        && bytes[0] == b'1'
        && matches!(bytes[1], b'3'..=b'9')
        && bytes.iter().all(u8::is_ascii_digit)
}

pub fn default_session_path() -> PathBuf {
    crate::config::get_home_dir()
        .join(".codex-session-delete")
        .join(SESSION_FILE)
}

fn normalize_api_base(value: &str) -> Result<String, String> {
    let value = value.trim().trim_end_matches('/');
    if !(value.starts_with("https://") || value.starts_with("http://")) {
        return Err("ClawKit 服务地址必须使用 HTTP 或 HTTPS".to_string());
    }
    Ok(value.to_string())
}

fn websocket_base(api_base: &str) -> String {
    api_base
        .strip_prefix("https://")
        .map(|value| format!("wss://{value}"))
        .or_else(|| {
            api_base
                .strip_prefix("http://")
                .map(|value| format!("ws://{value}"))
        })
        .unwrap_or_else(|| api_base.to_string())
}

fn response_message(body: &Value, status: StatusCode, fallback: &str) -> String {
    body.get("message")
        .or_else(|| body.get("detail"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{fallback} ({})", status.as_u16()))
}

fn session_status(session: &StoredSession) -> Value {
    json!({
        "status": "ok",
        "authenticated": true,
        "user": session.user,
        "device_id": session.device_id,
        "expires_at": session.expires_at,
    })
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(unix)]
fn restrict_session_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn restrict_session_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{default_session_path, login_request, normalize_api_base, websocket_base};

    #[test]
    fn production_endpoints_use_secure_schemes() {
        assert_eq!(
            normalize_api_base("https://image.clawkit.chat/"),
            Ok("https://image.clawkit.chat".into())
        );
        assert_eq!(websocket_base("https://clawkit.chat"), "wss://clawkit.chat");
        assert!(normalize_api_base("file:///tmp/socket").is_err());
    }

    #[test]
    fn shared_session_path_matches_codex_plus_plus() {
        assert!(default_session_path().ends_with(".codex-session-delete/clawkit-account.json"));
    }

    #[test]
    fn phone_login_uses_phone_password_endpoint() {
        let (endpoint, payload) = login_request(
            "https://image.clawkit.chat",
            "13812345678",
            "secret-value",
            "desktop-device",
        );

        assert_eq!(endpoint, "https://image.clawkit.chat/api/auth/phone/login");
        assert_eq!(payload["phone"], "13812345678");
        assert!(payload.get("username").is_none());
        assert_eq!(payload["product"], "codex-remote");
    }

    #[test]
    fn username_login_keeps_unified_login_endpoint() {
        let (endpoint, payload) = login_request(
            "https://image.clawkit.chat",
            "alice@example.com",
            "secret-value",
            "desktop-device",
        );

        assert_eq!(endpoint, "https://image.clawkit.chat/auth/login");
        assert_eq!(payload["username"], "alice@example.com");
        assert!(payload.get("phone").is_none());
    }
}
