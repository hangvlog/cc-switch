//! Bounded, account-scoped, temporary attachment externalization before model forwarding.
use super::{attachment_plan, ProxyError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::LazyLock,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

const LIMIT: usize = 40 << 20;
const MAX_RECEIPTS: usize = 512;
static PREPARATIONS: LazyLock<tokio::sync::Semaphore> =
    LazyLock::new(|| tokio::sync::Semaphore::new(2));
static CAPABILITIES: LazyLock<Mutex<HashMap<String, (Instant, Capabilities)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static LOCKS: LazyLock<Mutex<HashMap<String, std::sync::Weak<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .pool_max_idle_per_host(4)
        .build()
        .expect("attachment HTTP client")
});
#[derive(Clone, Deserialize)]
struct Capabilities {
    enabled: bool,
    models: Vec<String>,
    protocol: String,
    limit_bytes: usize,
    target_bytes: usize,
    max_attachment_bytes: usize,
}
#[derive(Clone, Deserialize, Serialize, Default)]
struct Receipt {
    id: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    expires: i64,
    #[serde(default)]
    upload_url: String,
    #[serde(default)]
    upload_headers: HashMap<String, Vec<String>>,
}
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
pub(crate) fn endpoint(upstream: &str) -> Option<String> {
    let url = url::Url::parse(upstream).ok()?;
    if url.username() != "" || url.password().is_some() {
        return None;
    }
    match url.host_str()? {
        "api.clawkit.chat" | "152.136.14.113" | "62.234.99.177" => {
            Some("https://api.clawkit.chat/v1/clawkit/attachments".into())
        }
        _ => None,
    }
}
fn cache_path(account: &str) -> std::path::PathBuf {
    crate::config::get_app_config_dir()
        .join("attachment-cache")
        .join(format!("{account}.json"))
}
fn read_cache(account: &str) -> HashMap<String, Receipt> {
    std::fs::metadata(cache_path(account))
        .ok()
        .filter(|m| m.len() <= 1 << 20)
        .and_then(|_| std::fs::read(cache_path(account)).ok())
        .and_then(|s| serde_json::from_slice(&s).ok())
        .unwrap_or_default()
}
async fn scoped_lock(key: &str) -> std::sync::Arc<Mutex<()>> {
    let mut locks = LOCKS.lock().await;
    if let Some(lock) = locks.get(key).and_then(std::sync::Weak::upgrade) {
        return lock;
    }
    locks.retain(|_, v| v.strong_count() > 0);
    let lock = std::sync::Arc::new(Mutex::new(()));
    locks.insert(key.into(), std::sync::Arc::downgrade(&lock));
    lock
}
fn safe_receipt(receipt: &Receipt, base: &str) -> bool {
    receipt.expires > now() + 300
        && receipt.url.starts_with(&format!("{base}/"))
        && receipt.url.len() <= 768
        && !receipt.id.is_empty()
        && url::Url::parse(&receipt.url).ok().zip(url::Url::parse(base).ok()).is_some_and(|(u,b)| u.origin() == b.origin() && u.path() == format!("{}/{}/content",b.path(),receipt.id) && u.fragment().is_none() && u.username().is_empty() && u.password().is_none())
}
async fn response_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, ProxyError> {
    let status = response.status();
    if !status.is_success() {
        return Err(ProxyError::AttachmentUpload(format!(
            "Attachment service returned {status}; model request was not sent"
        )));
    }
    let mut response = response;
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        ProxyError::AttachmentUpload("Cannot read attachment service response".into())
    })? {
        if data.len() + chunk.len() > 16384 {
            return Err(ProxyError::AttachmentUpload(
                "Attachment response exceeded its limit".into(),
            ));
        }
        data.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&data)
        .map_err(|_| ProxyError::AttachmentUpload("Invalid attachment service response".into()))
}
async fn authorize(
    base: &str,
    bearer: &str,
    attachment: &attachment_plan::Attachment,
) -> Result<Receipt, ProxyError> {
    response_json(CLIENT.post(format!("{base}/prepare")).header("Authorization",bearer).json(&json!({"sha256":attachment.hash,"size":attachment.bytes.len(),"mime":attachment.mime})).send().await.map_err(|_|ProxyError::AttachmentUpload("Cannot authorize attachment upload".into()))?).await
}
async fn upload(
    base: &str,
    bearer: &str,
    attachment: &attachment_plan::Attachment,
    account: &str,
) -> Result<Receipt, ProxyError> {
    let cache_key = format!("{}:{}", attachment.hash, attachment.mime);
    let lock = scoped_lock(&format!("{account}:{cache_key}")).await;
    let _guard = lock.lock().await;
    if let Some(receipt) = read_cache(account)
        .get(&cache_key)
        .filter(|r| safe_receipt(r, base))
    {
        return Ok(receipt.clone());
    }
    let mut receipt = authorize(base, bearer, attachment).await?;
    if !receipt.upload_url.is_empty() {
        let target = url::Url::parse(&receipt.upload_url)
            .map_err(|_| ProxyError::AttachmentUpload("Invalid upload authorization".into()))?;
        let host = target.host_str().unwrap_or("");
        if target.scheme() != "https"
            || !host.ends_with(".myqcloud.com")
            || target.username() != ""
            || target.password().is_some()
        {
            return Err(ProxyError::AttachmentUpload(
                "Unsupported upload authorization destination".into(),
            ));
        }
        let mut request = CLIENT.put(target).body(attachment.bytes.clone());
        for (name, values) in &receipt.upload_headers {
            for value in values {
                if name.eq_ignore_ascii_case("content-length") {
                    continue;
                }
                request = request.header(name, value);
            }
        }
        let response = request.send().await.map_err(|_| {
            ProxyError::AttachmentUpload(
                "Attachment upload failed; model request was not sent".into(),
            )
        })?;
        if !response.status().is_success() {
            return Err(ProxyError::AttachmentUpload(format!(
                "Attachment upload returned {}; model request was not sent",
                response.status()
            )));
        }
        receipt = response_json(
            CLIENT
                .post(format!("{base}/{}/complete", receipt.id))
                .header("Authorization", bearer)
                .send()
                .await
                .map_err(|_| {
                    ProxyError::AttachmentUpload("Attachment verification unavailable".into())
                })?,
        )
        .await?;
    }
    if !safe_receipt(&receipt, base) {
        return Err(ProxyError::AttachmentUpload(
            "Invalid or expiring attachment receipt".into(),
        ));
    }
    receipt.upload_url.clear();
    receipt.upload_headers.clear();
    let lock = scoped_lock(&format!("cache:{account}")).await;
    let _cache_guard = lock.lock().await;
    let mut cache = read_cache(account);
    cache.retain(|_, r| r.expires > now() + 300);
    if cache.len() >= MAX_RECEIPTS {
        if let Some(key) = cache
            .iter()
            .min_by_key(|(_, r)| r.expires)
            .map(|(k, _)| k.clone())
        {
            cache.remove(&key);
        }
    }
    cache.insert(cache_key, receipt.clone());
    if let Ok(data) = serde_json::to_vec(&cache) {
        let _ = crate::config::atomic_write_private(&cache_path(account), &data);
    }
    Ok(receipt)
}
pub(super) async fn prepare(
    body: &mut Value,
    serialized: Vec<u8>,
    upstream: &str,
    headers: &http::HeaderMap,
) -> Result<Vec<u8>, ProxyError> {
    if serialized.len() <= LIMIT || !crate::settings::get_settings().attachment_offload_enabled {
        return Ok(serialized);
    }
    let Some(base) = endpoint(upstream) else {
        return Ok(serialized);
    };
    tokio::time::timeout(
        Duration::from_secs(90),
        prepare_with_endpoint(body, serialized, &base, headers),
    )
    .await
    .map_err(|_| {
        ProxyError::AttachmentUpload(
            "Attachment preparation timed out; model request was not sent".into(),
        )
    })?
}
async fn prepare_with_endpoint(
    body: &mut Value,
    serialized: Vec<u8>,
    base: &str,
    headers: &http::HeaderMap,
) -> Result<Vec<u8>, ProxyError> {
    let _permit = PREPARATIONS.try_acquire().map_err(|_| {
        ProxyError::AttachmentUpload("Attachment preparation busy; retry later".into())
    })?;
    let Some(bearer) = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.starts_with("Bearer "))
    else {
        return Ok(serialized);
    };
    let account = format!("{:x}", Sha256::digest(bearer.as_bytes()));
    let capability_key = format!("{base}:{account}");
    let cached = CAPABILITIES
        .lock()
        .await
        .get(&capability_key)
        .filter(|(time, _)| time.elapsed() < Duration::from_secs(60))
        .map(|(_, c)| c.clone());
    let capabilities = match cached {
        Some(c) => c,
        None => {
            let c: Capabilities = response_json(
                CLIENT
                    .get(format!("{base}/capabilities"))
                    .header("Authorization", bearer)
                    .send()
                    .await
                    .map_err(|_| {
                        ProxyError::AttachmentUpload("Attachment capabilities unavailable".into())
                    })?,
            )
            .await?;
            let mut cache = CAPABILITIES.lock().await;
            if cache.len() >= 32 {
                cache.clear();
            }
            cache.insert(capability_key, (Instant::now(), c.clone()));
            c
        }
    };
    let model = body.get("model").and_then(Value::as_str).unwrap_or("");
    if !capabilities.enabled
        || capabilities.protocol != "responses"
        || !capabilities.models.iter().any(|m| m == model)
    {
        return Ok(serialized);
    }
    if capabilities.limit_bytes > LIMIT
        || capabilities.target_bytes >= capabilities.limit_bytes
        || capabilities.target_bytes < 1 << 20
        || capabilities.max_attachment_bytes > 20 << 20
    {
        return Err(ProxyError::AttachmentUpload(
            "Invalid attachment capability limits".into(),
        ));
    }
    if serialized.len() <= capabilities.limit_bytes {
        return Ok(serialized);
    }
    let original_bytes = serialized.len();
    let cache = read_cache(&account);
    let mut planned = attachment_plan::plan(body, capabilities.max_attachment_bytes)
        .map_err(ProxyError::AttachmentUpload)?;
    planned.sort_by_key(|a| {
        (
            std::cmp::Reverse(
                cache
                    .get(&format!("{}:{}", a.hash, a.mime))
                    .is_some_and(|r| safe_receipt(r, &base)),
            ),
            std::cmp::Reverse(a.saved),
        )
    });
    let mut remaining = serialized.len();
    let mut selected = Vec::new();
    for attachment in planned {
        if remaining <= capabilities.target_bytes {
            break;
        }
        remaining = remaining.saturating_sub(attachment.saved);
        selected.push(attachment);
    }
    if remaining > capabilities.target_bytes {
        return Err(ProxyError::RequestBodyTooLarge(original_bytes));
    }
    let start = Instant::now();
    let mut receipts = Vec::new();
    for pair in selected.chunks(2) {
        if pair.len() == 2 {
            let (first, second) = tokio::try_join!(
                upload(&base, bearer, &pair[0], &account),
                upload(&base, bearer, &pair[1], &account)
            )?;
            receipts.push((&pair[0], first));
            receipts.push((&pair[1], second));
        } else {
            receipts.push((&pair[0], upload(&base, bearer, &pair[0], &account).await?));
        }
    }
    // Commit replacements only after every upload is verified, preserving the original retry body.
    for (attachment, receipt) in receipts {
        attachment_plan::replace(body, attachment, &receipt.url)
            .map_err(ProxyError::AttachmentUpload)?;
    }
    let output = serde_json::to_vec(body)
        .map_err(|_| ProxyError::AttachmentUpload("Cannot serialize optimized request".into()))?;
    if output.len() > capabilities.target_bytes {
        return Err(ProxyError::RequestBodyTooLarge(output.len()));
    }
    log::info!(
        "[Attachments] externalized={} input_bytes={} output_bytes={} prepare_ms={}",
        selected.len(),
        original_bytes,
        output.len(),
        start.elapsed().as_millis()
    );
    super::attachment_log::report(
        original_bytes,
        output.len(),
        selected.len(),
        start.elapsed().as_millis() as u64,
    );
    Ok(output)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upload_credentials_only_go_to_known_gateway() {
        assert!(endpoint("https://api.clawkit.chat/v1/responses").is_some());
        assert!(endpoint("https://evil.test/v1/responses").is_none());
        assert!(endpoint("https://api.clawkit.chat.evil.test/v1/responses").is_none());
        assert!(endpoint("https://user:password@api.clawkit.chat/v1/responses").is_none());
    }
}

#[cfg(test)]
#[path = "attachment_tests.rs"]
mod integration_tests;
