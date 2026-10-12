//! ClawKit's deliberately small, versioned adapter to an existing Codex desktop.
//! Native creation shares the original initialized engine; no independent engine fallback.
mod attachments;
mod catalog;
mod clarification;
mod creation;
mod dispatch;
#[cfg(test)]
mod creation_tests;
mod file_uploads;
mod history;
mod ipc;
#[cfg(all(test, unix))]
mod ipc_tests;
mod ledger;
mod models;
pub mod native;
mod outbox;
mod queue;
mod saved_history;
mod sidebar;
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
    creations: creation::Creations,
    native_creations: native::creation::NativeCreations,
    queue: queue::Queue,
    attachments: attachments::Attachments,
    enabled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Bridge {
    pub fn open(home: &Path, storage: &Path) -> Result<Self, String> {
        Self::open_with_queue_storage(home, storage, storage)
    }

    pub fn open_with_queue_storage(
        home: &Path,
        storage: &Path,
        queue_storage: &Path,
    ) -> Result<Self, String> {
        if !home.is_absolute() {
            return Err("CODEX_HOME 必须是绝对路径".into());
        }
        let ledger = ledger::Ledger::open(storage)?;
        Ok(Self {
            home: home.into(),
            ledger,
            creations: creation::Creations::open(storage)?,
            native_creations: native::creation::NativeCreations::open(queue_storage)?,
            queue: queue::Queue::open(queue_storage)?,
            attachments: attachments::Attachments::open(queue_storage)?,
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
                let not_sent = (request["method"] == "desktop/thread/create"
                    && request["params"]["requestId"].as_str().is_some_and(|id| {
                        self.creations.not_sent(id) && self.native_creations.not_sent(id)
                    }))
                    || [
                        "desktop/turn/start",
                        "desktop/clarification/answer",
                        "desktop/queue/add",
                    ]
                    .iter()
                    .any(|m| request["method"] == *m)
                        && request["params"]["requestId"].as_str().is_some_and(|id| {
                            self.ledger.has_request(id) == Ok(false)
                                && self
                                    .queue
                                    .receipt(
                                        id,
                                        request["params"]["threadId"].as_str().unwrap_or(""),
                                    )
                                    .ok()
                                    .flatten()
                                    .is_none()
                        });
                json!({"id":id,"error":{"code":-32001,"message":message,"data":{"notSent":not_sent}}})
            }
        })
    }

    pub fn dispatch(&self, method: &str, params: &Value) -> Result<Value, String> {
        match method {
            "initialize" => Ok(json!({"userAgent":"clawkit-desktop-owner/1", "clawkit":{
                "mode":"desktop-owner","protocolVersion":1,"history":"snapshot","pollIntervalMs":8000,
                "historyPaging":true,"passiveRead":true,"newThread":native::ready(&self.home),"newThreadVia":"native-engine",
                "imageRead":true,"fileAttachments":true,"imageAttachments":true,"modelSelection":true,"clarifications":true,"messageQueue":true,"messageSteering":true,"interrupt":false,"approvals":false}})),
            "desktop/image/read" => self.attachments.read(params),
            "desktop/image/chunk" => self.attachments.chunk(params),
            "desktop/file/chunk" => self.attachments.file_chunk(params),
            "desktop/model/list" => models::list(&self.home),
            "desktop/thread/model" => {
                let thread = self.thread(params)?;
                let model = params["model"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 200)
                    .ok_or("请选择模型")?;
                models::select(&self.home, thread, model)
            }
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
            "desktop/thread/history" => saved_history::read(
                &self.home,
                self.thread(params)?,
                params["cursor"].as_str(),
                params["usersOnly"] == true,
                &self.attachments,
            ),
            "desktop/thread/read" | "desktop/thread/activate" => {
                let thread = self.thread(params)?;
                let snapshot = if method == "desktop/thread/activate" {
                    ipc::IpcClient::open_snapshot(&self.home, thread)?
                } else {
                    match ipc::IpcClient::passive_snapshot(&self.home, thread) {
                        Ok(snapshot) => snapshot,
                        Err(_) => {
                            return Ok(json!({"thread":{"id":thread,"turns":[],"desktop":{
                            "source":"history","status":"unknown","canSend":false,"needsActivation":true,"queue":self.queue.list(thread)?,
                            "reason":"发送时会在电脑打开此对话"}}}))
                        }
                    }
                };
                self.reconcile(thread, &snapshot.state)?;
                let mut visible = history::visible_thread(
                    &snapshot.state,
                    &snapshot.owner,
                    snapshot.revision,
                    &self.attachments,
                );
                if !version::can_send(&self.home) {
                    visible["desktop"]["canSend"] = json!(false);
                    visible["desktop"]["canSteer"] = json!(false);
                    visible["desktop"]["reason"] = json!("Codex 桌面连接尚未就绪，请重新连接");
                }
                if !self.ledger.unresolved(thread)?.is_empty() {
                    visible["desktop"]["canSend"] = json!(false);
                    visible["desktop"]["canSteer"] = json!(false);
                    visible["desktop"]["reason"] =
                        json!("上次发送结果未知，请在原桌面核对，不要重新发送");
                }
                visible["desktop"]["queue"] = json!(self.queue.list(thread)?);
                Ok(json!({"thread":visible}))
            }
            "desktop/queue/add" => self.enqueue(params),
            "desktop/queue/cancel" => self.cancel_queue(params),
            "desktop/clarification/answer" => self.answer(params),
            "desktop/turn/start" => self.start(params),
            "desktop/thread/create" => self.create(params),
            "desktop/creation/read" => {
                let id = request_id(params)?;
                let project = params["projectId"].as_str().ok_or("缺少项目 ID")?;
                if self.native_creations.exists(id) {
                    self.native_creations.read(&self.home, id, project)
                } else {
                    self.creations.read(&self.home, id, project)
                }
            }
            "desktop/dispatch/read" => {
                let thread = self.thread(params)?;
                let id = request_id(params)?;
                // Failure to read the desktop cannot turn an unknown result into rejection.
                if let Ok(mut client) = ipc::IpcClient::connect(&self.home) {
                    if let Ok(snapshot) = client.snapshot(thread, None) {
                        self.reconcile(thread, &snapshot.state)?;
                    }
                }
                if let Some(receipt) = self.queue.receipt(id, thread)? {
                    return Ok(receipt);
                }
                self.ledger.read(id, thread)
            }
            _ => Err("此操作不受原桌面接续支持".into()),
        }
    }

    fn create(&self, params: &Value) -> Result<Value, String> {
        let object = params.as_object().ok_or("新建参数无效")?;
        if object.keys().any(|key| {
            !["projectId", "requestId", "text", "images", "files"].contains(&key.as_str())
        }) {
            return Err("新建仅接受电脑项目和首条消息，模型与权限沿用 Codex 设置".into());
        }
        let id = request_id(params)?;
        let project_id = params["projectId"].as_str().ok_or("请选择电脑项目")?;
        let message = attachments::Message::parse(params)?;
        let text = &message.identity();
        let input = self.attachments.input(&message)?;
        if self.creations.same(id, project_id, text)? {
            return self.creations.read(&self.home, id, project_id);
        }
        self.native_creations
            .create(&self.home, id, project_id, text, input, &self.enabled)
    }

    #[cfg(target_os = "macos")]
    pub fn creation_preflight(&self) -> Result<(), String> {
        if native::ready(&self.home) {
            Ok(())
        } else {
            Err("请从 ClawKit 打开 Codex 后再新建".into())
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
