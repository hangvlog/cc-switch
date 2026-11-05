//! Only numeric preparation metrics leave the machine. URLs and attachment contents never do.
use serde_json::json;
use std::sync::LazyLock;
use tokio::sync::mpsc;
static SENDER: LazyLock<mpsc::Sender<(usize, usize, usize, u64)>> = LazyLock::new(|| {
    let (sender, mut receiver) = mpsc::channel::<(usize, usize, usize, u64)>(32);
    tauri::async_runtime::spawn(async move {
        let Ok(client) = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .build()
        else {
            return;
        };
        let endpoint = std::env::var("CLAWKIT_LOG_ENDPOINT")
            .unwrap_or_else(|_| "https://loghang.clawkit.chat/api/front/client-log/report".into());
        let session = uuid::Uuid::new_v4().to_string();
        while let Some((input, output, count, ms)) = receiver.recv().await {
            let _=client.post(&endpoint).json(&json!({"source":"clawkit-desktop","sessionId":session,"appVersion":env!("CARGO_PKG_VERSION"),"level":"info","type":"performance","page":"proxy/attachments","message":"Attachment request optimized","detail":json!({"inputBytes":input,"outputBytes":output,"externalized":count,"prepareMs":ms}).to_string()})).send().await;
        }
    });
    sender
});
pub(super) fn report(input: usize, output: usize, count: usize, ms: u64) {
    let _ = SENDER.try_send((input, output, count, ms));
}
