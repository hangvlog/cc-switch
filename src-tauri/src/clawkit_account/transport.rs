use serde_json::Value;

pub(super) fn http_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent(format!("ClawKit-Desktop/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(8))
        .timeout(std::time::Duration::from_secs(20))
}

pub(super) async fn post_json_with_direct_fallback(
    client: &reqwest::Client,
    direct_client: &reqwest::Client,
    endpoint: &str,
    payload: &Value,
) -> Result<reqwest::Response, String> {
    match client.post(endpoint).json(payload).send().await {
        Ok(response) => Ok(response),
        Err(error) if error.is_connect() || error.is_timeout() => direct_client
            .post(endpoint)
            .json(payload)
            .send()
            .await
            .map_err(|direct_error| account_connection_error(endpoint, &direct_error, true)),
        Err(error) => Err(account_connection_error(endpoint, &error, false)),
    }
}

fn account_connection_error(
    endpoint: &str,
    error: &reqwest::Error,
    retried_direct: bool,
) -> String {
    let host = reqwest::Url::parse(endpoint)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| "账号服务".to_string());
    let reason = if error.is_timeout() {
        "连接超时"
    } else if error.is_connect() {
        "网络、DNS、TLS 或代理连接失败"
    } else {
        "请求发送失败"
    };
    let retry = if retried_direct {
        "，通过系统网络失败后直连重试仍未成功"
    } else {
        ""
    };
    format!(
        "无法连接 ClawKit 账号服务（{host}）：{reason}{retry}。请检查系统代理、防火墙和 DNS 后重试"
    )
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use serde_json::json;

    use super::http_client_builder;
    use crate::clawkit_account::ClawkitAccountClient;

    #[tokio::test]
    async fn login_retries_directly_when_the_configured_proxy_is_unreachable() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let server_address = server.local_addr().unwrap();
        let responder = thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            let body = r#"{"code":200,"data":{"token":"test-token","expires_in":3600,"user":{"username":"alice"}}}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });

        let unavailable_proxy = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy_address = unavailable_proxy.local_addr().unwrap();
        drop(unavailable_proxy);
        let proxied_client = http_client_builder()
            .proxy(reqwest::Proxy::all(format!("http://{proxy_address}")).unwrap())
            .build()
            .unwrap();
        let direct_client = http_client_builder().no_proxy().build().unwrap();
        let session_dir = tempfile::tempdir().unwrap();
        let client = ClawkitAccountClient {
            account_api_base: format!("http://{server_address}"),
            relay_api_base: format!("http://{server_address}"),
            session_path: session_dir.path().join("clawkit-account.json"),
            client: proxied_client,
            direct_client,
        };

        let result = client.login("alice", "secret-value").await.unwrap();

        assert_eq!(result["authenticated"], true);
        assert_eq!(result["user"], json!({ "username": "alice" }));
        responder.join().unwrap();
    }
}
