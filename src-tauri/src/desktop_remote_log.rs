//! Only fixed lifecycle labels leave the machine; never pass IPC errors or payloads here.
use once_cell::sync::Lazy;
use serde_json::json;
use std::sync::Mutex;
use tokio::sync::mpsc;

static SENDER: Lazy<Mutex<Option<mpsc::Sender<&'static str>>>> = Lazy::new(|| Mutex::new(None));

pub fn report(phase: &str) {
    let event = match phase {
        "connected" => "desktop_remote_connected",
        "reconnecting" => "desktop_remote_reconnecting",
        "login-required" => "desktop_remote_login_required",
        "disabled" => "desktop_remote_disabled",
        _ => return,
    };
    let Ok(mut stored) = SENDER.lock() else {
        return;
    };
    let sender = stored.get_or_insert_with(|| {
        let (sender, mut receiver) = mpsc::channel::<&'static str>(32);
        tauri::async_runtime::spawn(async move {
            let Ok(client) = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(3))
                .build()
            else {
                return;
            };
            let session = uuid::Uuid::new_v4().to_string();
            let endpoint = std::env::var("CLAWKIT_LOG_ENDPOINT").unwrap_or_else(|_| {
                option_env!("CLAWKIT_LOG_ENDPOINT")
                    .unwrap_or("https://loghang.clawkit.chat/api/front/client-log/report")
                    .into()
            });
            while let Some(event) = receiver.recv().await {
                let _ = client
                    .post(&endpoint)
                    .json(&json!({"source":"clawkit-desktop","sessionId":session,
                    "level":"info","type":"custom","page":"remote","message":event}))
                    .send()
                    .await;
            }
        });
        sender
    });
    let _ = sender.try_send(event);
}
