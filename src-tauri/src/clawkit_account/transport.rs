use serde_json::Value;

pub(crate) fn http_client_builder() -> reqwest::ClientBuilder {
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

pub(crate) async fn post_json_with_bearer_direct_fallback(
    client: &reqwest::Client,
    direct_client: &reqwest::Client,
    endpoint: &str,
    bearer_token: &str,
    payload: &Value,
) -> Result<reqwest::Response, String> {
    match client
        .post(endpoint)
        .bearer_auth(bearer_token)
        .json(payload)
        .send()
        .await
    {
        Ok(response) => Ok(response),
        Err(error) if error.is_connect() || error.is_timeout() => {
            log_connection_retry("ClawKit API 代理", endpoint, &error);
            direct_client
                .post(endpoint)
                .bearer_auth(bearer_token)
                .json(payload)
                .send()
                .await
                .map_err(|direct_error| {
                    connection_error("ClawKit API 代理", endpoint, &direct_error, true)
                })
        }
        Err(error) => Err(connection_error(
            "ClawKit API 代理",
            endpoint,
            &error,
            false,
        )),
    }
}

fn log_connection_retry(service_name: &str, endpoint: &str, error: &reqwest::Error) {
    let host = reqwest::Url::parse(endpoint)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| service_name.to_string());
    log::warn!(
        "{service_name} request to {host} failed through system networking; retrying directly: {error}"
    );
}

fn account_connection_error(
    endpoint: &str,
    error: &reqwest::Error,
    retried_direct: bool,
) -> String {
    connection_error("ClawKit 账号服务", endpoint, error, retried_direct)
}

fn connection_error(
    service_name: &str,
    endpoint: &str,
    error: &reqwest::Error,
    retried_direct: bool,
) -> String {
    let host = reqwest::Url::parse(endpoint)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| service_name.to_string());
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
    log::warn!(
        "{service_name} request to {host} failed{}: {error}",
        if retried_direct {
            " after direct retry"
        } else {
            ""
        }
    );
    format!(
        "无法连接 {service_name}（{host}）：{reason}{retry}。请检查系统代理、防火墙和 DNS 后重试"
    )
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use serde_json::json;

    use super::{http_client_builder, post_json_with_bearer_direct_fallback};
    use crate::clawkit_account::{ClawkitAccountClient, StoredSession};

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

    #[tokio::test]
    async fn relogin_preserves_device_after_logout_expiry_and_unauthorized_ticket() {
        for cleanup in ["logout", "expiry", "unauthorized"] {
            let server = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}", server.local_addr().unwrap());
            let responder = thread::spawn(move || {
                if cleanup == "unauthorized" {
                    let (mut stream, _) = server.accept().unwrap();
                    let _ = stream.read(&mut [0_u8; 4096]);
                    write!(stream, "HTTP/1.1 401 Unauthorized\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").unwrap();
                }
                let (mut stream, _) = server.accept().unwrap();
                let _ = stream.read(&mut [0_u8; 4096]);
                let body = r#"{"code":200,"data":{"token":"new-token","expires_in":3600,"user":{"id":8}}}"#;
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            });
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("clawkit-account.json");
            // Write the old-format session directly to exercise upgrade migration.
            std::fs::write(
                &path,
                serde_json::to_vec(&StoredSession {
                    token: "old-token".into(),
                    user: json!({ "id": 7 }),
                    device_id: "clawkit-desktop-existing".into(),
                    expires_at: if cleanup == "expiry" { 1 } else { u64::MAX },
                })
                .unwrap(),
            )
            .unwrap();
            let client = ClawkitAccountClient::with_endpoints(&endpoint, &endpoint, &path).unwrap();
            match cleanup {
                "expiry" => assert_eq!(client.status()["authenticated"], false),
                "unauthorized" => assert!(client.create_owner_socket_ticket().await.is_err()),
                _ => {
                    client.logout().unwrap();
                }
            }
            assert!(!path.exists(), "invalid credentials must be removed");
            // New client models a desktop restart, and login uses another account.
            let restarted =
                ClawkitAccountClient::with_endpoints(&endpoint, &endpoint, &path).unwrap();
            let status = restarted
                .login("other-account", "test-password")
                .await
                .unwrap();
            responder.join().unwrap();
            assert_eq!(status["deviceId"], "clawkit-desktop-existing", "{cleanup}");
            assert_eq!(status["user"]["id"], 8);
            use sha2::{Digest, Sha256};
            assert_eq!(
                restarted.remote_queue_scope().unwrap(),
                format!("{:x}", Sha256::digest(b"8"))
            );
            let stored = std::fs::read_to_string(&path).unwrap();
            assert!(!stored.contains("old-token"));
            let identity = std::fs::read_to_string(path.with_extension("device-id")).unwrap();
            assert_eq!(identity, "clawkit-desktop-existing");
        }
    }

    #[tokio::test]
    async fn gateway_bootstrap_retries_directly_and_keeps_bearer_auth() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let server_address = server.local_addr().unwrap();
        let responder = thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            let mut request = [0_u8; 4096];
            let size = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..size]);
            assert!(request
                .to_ascii_lowercase()
                .contains("authorization: bearer test-token"));
            assert!(request.contains(r#""device_id":"test-device""#));
            let body = r#"{"success":true,"data":{}}"#;
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
        let response = post_json_with_bearer_direct_fallback(
            &proxied_client,
            &direct_client,
            &format!("http://{server_address}/api/user/clawkit/codex/bootstrap"),
            "test-token",
            &json!({ "device_id": "test-device" }),
        )
        .await
        .unwrap();

        assert!(response.status().is_success());
        responder.join().unwrap();
    }
}
