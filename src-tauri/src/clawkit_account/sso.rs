use super::*;
use tauri_plugin_opener::OpenerExt;

impl ClawkitAccountClient {
    pub async fn login_sso(&self, app: &tauri::AppHandle) -> Result<Value, String> {
        let device_id = identity::load_or_create(&self.session_path)?;
        let endpoint = format!("{}/api/codex-remote/sso/start", self.account_api_base);
        let response = post_json_with_direct_fallback(&self.client, &self.direct_client, &endpoint,
            &json!({"device_id":device_id})).await?;
        let status = response.status();
        let flow: Value = response.json().await.map_err(|_| "统一登录响应无效")?;
        if !status.is_success() { return Err(response_message(&flow,status,"统一登录失败")); }
        let url = flow["login_url"].as_str().ok_or("统一登录地址缺失")?;
        let parsed = reqwest::Url::parse(url).map_err(|_| "统一登录地址无效")?;
        let issuer = reqwest::Url::parse(&self.account_api_base).map_err(|_| "统一登录配置无效")?;
        if parsed.origin()!=issuer.origin() || parsed.path()!="/auth" { return Err("统一登录地址不匹配".into()); }
        app.opener().open_url(url,None::<String>).map_err(|_| "无法打开浏览器，请重试")?;
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(300);
        while std::time::Instant::now()<deadline {
            let response=post_json_with_direct_fallback(&self.client,&self.direct_client,
                &format!("{}/api/codex-remote/sso/poll",self.account_api_base),
                &json!({"state":flow["state"],"poll_token":flow["poll_token"]})).await?;
            let status=response.status();
            let data:Value=response.json().await.map_err(|_| "统一登录响应无效")?;
            if !status.is_success(){return Err(response_message(&data,status,"统一登录失败"));}
            if data["token"].as_str().is_some_and(|s| !s.is_empty()) {
                return self.accept_login(&data, device_id, false);
            }
            if data["pending"]!=true{return Err("统一登录响应无效".into());}
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        }
        Err("统一登录超时，请重试".into())
    }

    pub fn sso_logout_url(&self)->String {
        let mut url=reqwest::Url::parse(&format!("{}/logout",self.account_api_base)).expect("validated issuer");
        url.query_pairs_mut().append_pair("client_id","clawkit-remote")
            .append_pair("post_logout_redirect_uri",&format!("{}/api/codex-remote/sso/logged-out",self.account_api_base));
        url.into()
    }
}
