//! ClawKit's deliberately small, versioned adapter to an existing Codex desktop.
//! No app-server fallback, no arbitrary RPC passthrough, no writes to Codex files.
mod catalog;
mod history;
mod ipc;
#[cfg(all(test, unix))]
mod ipc_tests;
mod ledger;
mod version;

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub struct Bridge {
    home: PathBuf,
    ledger: ledger::Ledger,
    enabled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Bridge {
    pub fn open(home: &Path, storage: &Path) -> Result<Self, String> {
        if !home.is_absolute() {
            return Err("CODEX_HOME 必须是绝对路径".into());
        }
        Ok(Self {
            home: home.into(),
            ledger: ledger::Ledger::open(storage)?,
            enabled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        })
    }

    pub fn enable_flag(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.enabled.clone()
    }

    /// Caller serializes requests per device. IDs are opaque and must be echoed unchanged.
    pub fn handle(&self, request: Value) -> Option<Value> {
        let id = request.get("id")?.clone();
        if !(id.is_string() || id.is_i64() || id.is_u64()) {
            return None;
        }
        let result = self.dispatch(request["method"].as_str().unwrap_or(""), &request["params"]);
        Some(match result {
            Ok(value) => json!({"id":id,"result":value}),
            Err(message) => {
                let not_sent = request["method"] == "desktop/turn/start"
                    && request["params"]["requestId"]
                        .as_str()
                        .is_some_and(|id| self.ledger.has_request(id) == Ok(false));
                json!({"id":id,"error":{"code":-32001,"message":message,"data":{"notSent":not_sent}}})
            }
        })
    }

    pub fn dispatch(&self, method: &str, params: &Value) -> Result<Value, String> {
        match method {
            "initialize" => Ok(json!({"userAgent":"clawkit-desktop-owner/1", "clawkit":{
                "mode":"desktop-owner","protocolVersion":1,"history":"snapshot","pollIntervalMs":8000,
                "newThread":false,"interrupt":false,"approvals":false}})),
            "desktop/status" => Ok(version::status(&self.home)),
            "desktop/thread/list" => {
                let offset = params["cursor"]
                    .as_str()
                    .unwrap_or("0")
                    .parse::<usize>()
                    .map_err(|_| "分页游标无效")?;
                if offset > 100_000 {
                    return Err("分页游标超出范围".into());
                }
                catalog::list(&self.home, offset)
            }
            "desktop/thread/read" => {
                let thread = self.thread(params)?;
                let mut client = ipc::IpcClient::connect(&self.home)?;
                let snapshot = client.snapshot(thread, None)?;
                self.reconcile(thread, &snapshot.state)?;
                let mut visible =
                    history::visible_thread(&snapshot.state, &snapshot.owner, snapshot.revision);
                if !version::can_send(&self.home) {
                    visible["desktop"]["canSend"] = json!(false);
                    visible["desktop"]["reason"] = json!("当前 Codex 版本尚未验证发送兼容性");
                }
                if !self.ledger.unresolved(thread)?.is_empty() {
                    visible["desktop"]["canSend"] = json!(false);
                    visible["desktop"]["reason"] =
                        json!("上次发送结果未知，请在原桌面核对，不要重新发送");
                }
                Ok(json!({"thread":visible}))
            }
            "desktop/turn/start" => self.start(params),
            "desktop/dispatch/read" => {
                let thread = self.thread(params)?;
                let id = request_id(params)?;
                // Failure to read the desktop cannot turn an unknown result into rejection.
                if let Ok(mut client) = ipc::IpcClient::connect(&self.home) {
                    if let Ok(snapshot) = client.snapshot(thread, None) {
                        self.reconcile(thread, &snapshot.state)?;
                    }
                }
                self.ledger.read(id, thread)
            }
            _ => Err("原桌面接续仅支持读取和空闲任务续聊；此操作不受支持".into()),
        }
    }

    fn thread<'a>(&self, params: &'a Value) -> Result<&'a str, String> {
        let thread = params["threadId"].as_str().ok_or("缺少任务 ID")?;
        catalog::verify_thread(&self.home, thread)?;
        Ok(thread)
    }

    fn reconcile(&self, thread: &str, state: &Value) -> Result<(), String> {
        for id in self.ledger.unresolved(thread)? {
            if history::contains_request(state, &id) {
                self.ledger.mark(&id, "accepted")?;
            }
        }
        Ok(())
    }

    fn start(&self, params: &Value) -> Result<Value, String> {
        // Reject settings overrides: the original owner's settings are authoritative.
        let object = params.as_object().ok_or("发送参数无效")?;
        if object
            .keys()
            .any(|key| !["threadId", "requestId", "text"].contains(&key.as_str()))
        {
            return Err("续聊不能覆盖原任务的模型、权限或工作目录".into());
        }
        let thread = self.thread(params)?;
        let id = request_id(params)?;
        let text = params["text"]
            .as_str()
            .filter(|text| !text.trim().is_empty() && text.len() <= 16_000)
            .ok_or("消息不能为空且最多 16000 字节")?;
        // Duplicates return their durable receipt even while the desktop is offline/busy.
        if let Some(receipt) = self.ledger.existing(id, thread, text)? {
            return Ok(receipt);
        }
        if !version::can_send(&self.home) {
            return Err("当前 Codex 版本或系统尚未验证发送兼容性".into());
        }
        let mut client = ipc::IpcClient::connect(&self.home)?;
        let initial = client.snapshot(thread, None)?;
        self.reconcile(thread, &initial.state)?;
        if let Some(reason) = history::send_block_reason(&initial.state) {
            return Err(reason.into());
        }
        let checked = client.snapshot(thread, Some(&initial.owner))?;
        if let Some(reason) = history::send_block_reason(&checked.state) {
            return Err(reason.into());
        }
        if checked.revision != initial.revision {
            return Err("原任务刚刚更新，请刷新后发送".into());
        }
        if let Some(receipt) = self.ledger.begin(id, thread, text)? {
            return Ok(receipt);
        }
        if !self.enabled.load(std::sync::atomic::Ordering::SeqCst) {
            self.ledger.mark(id, "rejected")?;
            return self.ledger.read(id, thread);
        }
        // Once written, any transport or schema error is unknown, never auto-retry.
        let result = client.start_turn(thread, &checked.owner, id, text);
        let status = match result {
            Ok(response)
                if response["result"]["result"].is_object()
                    && !response["result"]["result"]["turn"].is_null() =>
            {
                "accepted"
            }
            _ => "unknown",
        };
        self.ledger.mark(id, status)?;
        if status == "unknown" {
            // A failed framed read may have consumed a partial frame. Reconnect before reading.
            if let Ok(mut fresh) = ipc::IpcClient::connect(&self.home) {
                if let Ok(snapshot) = fresh.snapshot(thread, None) {
                    self.reconcile(thread, &snapshot.state)?;
                }
            }
        }
        self.ledger.read(id, thread)
    }
}

fn request_id(params: &Value) -> Result<&str, String> {
    let id = params["requestId"].as_str().ok_or("缺少发送请求编号")?;
    uuid::Uuid::parse_str(id).map_err(|_| "发送请求编号必须为 UUID")?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_capabilities_and_no_arbitrary_rpc() {
        let root = tempfile::tempdir().unwrap();
        let bridge = Bridge::open(root.path(), &root.path().join("ledger")).unwrap();
        assert_eq!(
            bridge.dispatch("initialize", &json!({})).unwrap()["clawkit"]["mode"],
            "desktop-owner"
        );
        for method in [
            "thread/start",
            "thread/resume",
            "turn/start",
            "turn/interrupt",
            "exec",
        ] {
            assert!(bridge.dispatch(method, &json!({})).is_err());
        }
        assert!(bridge.handle(json!({"method":"initialized"})).is_none());
        assert_eq!(
            bridge
                .handle(json!({"id":"phone:1","method":"initialize"}))
                .unwrap()["id"],
            "phone:1"
        );
    }
}
