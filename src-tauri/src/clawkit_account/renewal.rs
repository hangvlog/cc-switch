use super::*;
use super::transport::post_json_with_bearer_direct_fallback;

static RENEWAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn storage_lock(path: &Path) -> Result<std::fs::File, String> {
    let parent = path.parent().ok_or("登录存储路径无效")?;
    fs::create_dir_all(parent).map_err(|_| "登录存储不可用")?;
    for _ in 0..100 {
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(windows)]
        { use std::os::windows::fs::OpenOptionsExt; options.share_mode(0); }
        if let Ok(file) = options.open(path.with_extension("renew-lock")) {
            #[cfg(unix)]
            {
                use std::os::fd::AsRawFd;
                if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 { return Ok(file); }
            }
            #[cfg(windows)]
            { return Ok(file); }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Err("登录续期正在处理中，请稍后重试".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn client(status: axum::http::StatusCode) -> (ClawkitAccountClient, tempfile::TempDir, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base=format!("http://{}",listener.local_addr().unwrap());
        let app=axum::Router::new().route("/api/codex-remote/sso/refresh",axum::routing::post(move |axum::Json(body):axum::Json<Value>|async move {
            assert_eq!(body["device_id"],"desktop-test");
            (status,axum::Json(json!({"token":"renewed-access","user":{"id":42,"sub":"42"},"expires_in":86400,
                "refresh_token":"durable-refresh","refresh_expires_at":unix_timestamp()+86400*30})))
        }));
        let task=tokio::spawn(async move {axum::serve(listener,app).await.unwrap();});
        let dir=tempfile::tempdir().unwrap();
        let client=ClawkitAccountClient::with_endpoints(&base,&base,dir.path().join("session.json")).unwrap();
        client.save_session(&StoredSession{password_login:false,token:"expired-access".into(),user:json!({"id":42,"sub":"42"}),device_id:"desktop-test".into(),
            expires_at:1,refresh_token:Some("durable-refresh".into()),refresh_expires_at:Some(unix_timestamp()+86400*30)}).unwrap();
        (client,dir,task)
    }

    #[tokio::test]
    async fn expired_access_recovers_without_browser_and_concurrent_refresh_is_single_flight() {
        let (account,_dir,task)=client(axum::http::StatusCode::OK).await;
        assert!(account.load_active_session().is_err());
        assert!(account.load_session().unwrap().can_refresh());
        let (a,b)=tokio::join!(account.ensure_session(),account.ensure_session());
        assert_eq!(a,Ok(true));assert_eq!(b,Ok(false));
        assert_eq!(account.active_credentials().unwrap(),("renewed-access".into(),"desktop-test".into()));
        task.abort();
    }

    #[tokio::test]
    async fn temporary_error_keeps_refresh_but_revocation_clears_login() {
        let (account,_dir,task)=client(axum::http::StatusCode::SERVICE_UNAVAILABLE).await;
        assert!(account.ensure_session().await.is_err());assert!(account.load_session().unwrap().can_refresh());task.abort();
        let (account,_dir,task)=client(axum::http::StatusCode::UNAUTHORIZED).await;
        assert!(account.ensure_session().await.unwrap_err().contains("重新登录"));assert!(!account.session_path.exists());
        assert_eq!(identity::load_or_create(&account.session_path).unwrap(),"desktop-test");task.abort();
    }
}

impl StoredSession {
    pub(super) fn can_refresh(&self) -> bool {
        self.refresh_token.as_ref().is_some_and(|s| !s.is_empty())
            && self.refresh_expires_at.unwrap_or(0) > unix_timestamp()
    }
}

impl ClawkitAccountClient {
    /// Called by the independent service as well as ticket creation. No browser/UI required.
    pub async fn ensure_session(&self) -> Result<bool, String> {
        let _guard = RENEWAL.lock().await;
        let _file = storage_lock(&self.session_path).await?;
        let current = self.load_session()?;
        if current.can_refresh() && current.expires_at > unix_timestamp() + 300 {
            return Ok(false);
        }
        let (path, body) = match &current.refresh_token {
            Some(token) => ("refresh", json!({"device_id":current.device_id,"refresh_token":token})),
            None if current.expires_at > unix_timestamp() => ("renew", json!({"device_id":current.device_id})),
            _ => return Err("登录已过期，请重新登录".into()),
        };
        let endpoint = format!("{}/api/codex-remote/sso/{path}", self.account_api_base);
        let response = post_json_with_bearer_direct_fallback(&self.client, &self.direct_client,
            &endpoint, &current.token, &body).await?;
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            if self.load_session().is_ok_and(|s| s.token == current.token) { self.logout()?; }
            return Err("远程登录已过期或被撤销，请重新登录".into());
        }
        if !status.is_success() { return Err(format!("登录续期暂不可用 ({status})")); }
        let data: Value = response.json().await.map_err(|_| "登录续期响应无效")?;
        if data["user"]["id"] != current.user["id"]
            || data["user"]["sub"].as_str() != current.user["id"].as_u64().map(|v|v.to_string()).as_deref() {
            return Err("登录续期账号不一致".into());
        }
        let next = StoredSession {
            password_login: current.password_login,
            token: data["token"].as_str().filter(|v|!v.is_empty()).ok_or("续期令牌缺失")?.into(),
            user:data["user"].clone(), device_id:current.device_id.clone(),
            expires_at:unix_timestamp()+data["expires_in"].as_u64().filter(|v|*v>0 && *v<=86400).ok_or("续期时间无效")?,
            refresh_token:Some(data["refresh_token"].as_str().filter(|v|!v.is_empty()).ok_or("续期凭据缺失")?.into()),
            refresh_expires_at:Some(data["refresh_expires_at"].as_f64().ok_or("续期有效期缺失")? as u64),
        };
        // A logout/account change while HTTP was pending must never restore the old login.
        let saved = self.load_session()?;
        if saved.token != current.token || saved.device_id != current.device_id { return Ok(false); }
        self.save_session(&next)?;
        Ok(true)
    }

    pub async fn revoke_session(&self) -> Result<(), String> {
        let _guard = RENEWAL.lock().await;
        let _file = storage_lock(&self.session_path).await?;
        let current = self.load_session()?;
        let (base,path,body) = if let Some(refresh) = &current.refresh_token {
            (&self.account_api_base,"sso/logout",json!({"device_id":current.device_id,"refresh_token":refresh}))
        } else { (&self.relay_api_base,"account/logout",json!({})) };
        let response = post_json_with_bearer_direct_fallback(&self.client,&self.direct_client,
            &format!("{base}/api/codex-remote/{path}"),&current.token,&body).await?;
        if !response.status().is_success() { return Err("服务器未确认退出，请联网后重试".into()); }
        self.logout()?;
        Ok(())
    }
}
