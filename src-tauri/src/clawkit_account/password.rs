use super::*;

impl ClawkitAccountClient {
    pub async fn login_password(&self, username: &str, password: &str) -> Result<Value, String> {
        if username.trim().is_empty() || password.is_empty() {
            return Err("请输入账号和密码".into());
        }
        let device_id = identity::load_or_create(&self.session_path)?;
        let endpoint = format!("{}/api/codex-remote/sso/password", self.account_api_base);
        let response = post_json_with_direct_fallback(&self.client, &self.direct_client, &endpoint,
            &json!({"username":username.trim(), "password":password, "device_id":device_id})).await?;
        let status = response.status();
        let data: Value = response.json().await.map_err(|_| "登录响应无效")?;
        if !status.is_success() { return Err(response_message(&data, status, "登录失败")); }
        self.accept_login(&data, device_id, true)
    }

    pub fn uses_browser_login(&self) -> bool {
        self.load_session().is_ok_and(|session| !session.password_login)
    }

    pub(super) fn accept_login(&self, data: &Value, device_id: String, password_login: bool) -> Result<Value, String> {
        let token = data["token"].as_str().filter(|s| !s.is_empty()).ok_or("登录令牌缺失")?;
        let id = data["user"]["id"].as_u64().ok_or("统一账号 ID 缺失")?;
        if data["user"]["sub"].as_str() != Some(id.to_string().as_str()) {
            return Err("统一账号身份不一致".into());
        }
        let session = StoredSession {
            password_login, token: token.into(), user: data["user"].clone(), device_id,
            expires_at: unix_timestamp() + data["expires_in"].as_u64().filter(|s| *s > 0 && *s <= 86400).ok_or("登录有效期无效")?,
            refresh_token: Some(data["refresh_token"].as_str().filter(|s| !s.is_empty()).ok_or("续期凭据缺失")?.into()),
            refresh_expires_at: Some(data["refresh_expires_at"].as_f64().filter(|v| *v > unix_timestamp() as f64).ok_or("续期有效期无效")? as u64),
        };
        self.save_session(&session)?;
        Ok(session_status(&session))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn password_login_stores_only_session_and_rejects_wrong_credentials() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new().route("/api/codex-remote/sso/password", axum::routing::post(
            |axum::Json(body): axum::Json<Value>| async move {
                assert_eq!(body["username"], "native-test");
                assert!(body["device_id"].as_str().unwrap().len() >= 8);
                if body["password"] != "fixture-password" {
                    return (axum::http::StatusCode::UNAUTHORIZED, axum::Json(json!({"detail":"账号或密码错误"})));
                }
                (axum::http::StatusCode::OK, axum::Json(json!({"token":"fixture-access",
                    "user":{"id":42,"sub":"42"},"expires_in":86400,"refresh_token":"fixture-refresh",
                    "refresh_expires_at":unix_timestamp()+86400*30})))
            }));
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        let dir = tempfile::tempdir().unwrap();
        let account = ClawkitAccountClient::with_endpoints(&base, &base, dir.path().join("session.json")).unwrap();
        assert!(account.login_password(" native-test ", "wrong").await.unwrap_err().contains("账号或密码错误"));
        assert!(!account.session_path.exists());
        let status = account.login_password(" native-test ", "fixture-password").await.unwrap();
        assert_eq!(status["authenticated"], true);
        let saved = fs::read_to_string(&account.session_path).unwrap();
        assert!(!saved.contains("fixture-password"));
        assert!(account.load_session().unwrap().can_refresh());
        assert!(!account.uses_browser_login());
        let device = account.load_session().unwrap().device_id;
        assert!(account.accept_login(&json!({"token":"bad","user":{"id":42,"sub":"43"}}), device, true).is_err());
        assert_eq!(account.load_session().unwrap().token, "fixture-access");
        task.abort();
    }
}
