//! ClawKit's deliberately small, versioned adapter to an existing Codex desktop.
//! Native creation shares the original initialized engine; no independent engine fallback.
mod attachments;
mod image_preview;
mod activity;
mod activity_automations;
mod catalog;
mod catalog_reader;
pub use catalog_reader::CatalogReader;
mod conditional_read;
mod clarification;
mod creation;
mod dispatch;
#[cfg(test)]
mod creation_tests;
mod file_uploads;
mod history;
mod latest_message;
mod ipc;
#[cfg(all(test, unix))]
mod ipc_tests;
mod ledger;
mod models;
mod notifications;
mod notification_metadata;
#[cfg(unix)]
mod router;
#[cfg(unix)]
mod router_launch;
#[cfg(all(test, unix))]
mod router_tests;
pub mod native;
#[cfg(target_os="macos")]
mod renderer;
mod outbox;
mod owner_control;
mod owner_output;
mod owner_patches;
pub mod owner_stream;
mod queue;
mod saved_history;
mod search;
mod sidebar;
mod thread_status;
mod task_files;
mod version;
mod unread;
#[cfg(target_os="macos")]
mod launch_cli;

#[cfg(unix)]
pub fn maintain_background_router(home: &Path, executable: &Path) -> Result<(), String> {
    router_launch::maintain(home, executable)
}

pub fn maybe_run_router() {
    #[cfg(target_os="macos")]
    renderer::maybe_run();
    #[cfg(target_os="macos")]
    launch_cli::maybe_run();
    #[cfg(unix)]
    router_launch::maybe_run();
}

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub struct Bridge {
    owner_streams: owner_stream::OwnerStreams,
    output_storage: PathBuf,
    notifications: std::sync::Mutex<notifications::Notifications>,
    home: PathBuf,
    search: search::Search,
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
        let enabled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        Ok(Self {
            owner_streams: owner_stream::OwnerStreams::new(enabled.clone()),
            output_storage: queue_storage.into(),
            notifications: std::sync::Mutex::new(notifications::Notifications::default()),
            home: home.into(),
            ledger,
            creations: creation::Creations::open(storage)?,
            native_creations: native::creation::NativeCreations::open(queue_storage)?,
            queue: queue::Queue::open(queue_storage)?,
            attachments: attachments::Attachments::open(queue_storage)?,
            search: search::Search::open(queue_storage)?,
            enabled,
        })
    }

    pub fn owner_streams(&self) -> owner_stream::OwnerStreams { self.owner_streams.clone() }

    pub fn catalog_reader(&self) -> CatalogReader {
        CatalogReader::new(&self.home, self.enabled.clone())
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
        if method != "initialize" && !self.enabled.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("远程连接已关闭".into());
        }
        match method {
            "initialize" => { let event_ready=native::event_ready(&self.home); let owner_ready=version::can_send(&self.home); Ok(json!({"userAgent":"clawkit-desktop-owner/1", "clawkit":{
                "mode":"desktop-owner","protocolVersion":1,"history":"snapshot","pollIntervalMs":8000,
                "notifications":true,"readState":cfg!(any(unix,windows)),"conditionalRead":true,"threadStatuses":true,"userMessageSearch":true,"historyPaging":true,"passiveRead":true,"newThread":native::ready(&self.home),"newThreadVia":"native-engine","eventStream":event_ready,"ownerStream":!event_ready&&owner_ready,
                "taskFileRead":true,"imageRead":true,"imagePreview":true,"fileAttachments":true,"imageAttachments":true,"modelSelection":true,"clarifications":true,"messageQueue":true,"messageSteering":true,"interrupt":event_ready||owner_ready,"approvals":event_ready||owner_ready}})) },
            "desktop/events/read" => {
                let mut batch=native::read_events(&self.home,params.clone())?;
                self.project_events(&mut batch);Ok(batch)
            },
            "desktop/owner/watch" => self.owner_streams.watch(&self.home,&self.output_storage,self.thread(params)?,params["watchId"].as_str().ok_or("Missing watch ID")?,params["latestOnly"]==true),
            "desktop/owner/unwatch" => { self.owner_streams.unwatch(self.thread(params)?,params["watchId"].as_str().ok_or("Missing watch ID")?); Ok(json!({"ok":true})) },
            "desktop/owner/respond" => {
                owner_control::respond(&self.home,self.thread(params)?,params)
            }
            "desktop/request/respond" => {
                self.thread(params)?;
                native::request(&self.home, "clawkit/request/respond", params.clone())
            }
            "desktop/turn/interrupt" => {
                self.thread(params)?;
                let turn = params["turnId"].as_str().filter(|v|!v.is_empty() && v.len()<200).ok_or("缺少执行轮次")?;
                if native::event_ready(&self.home) {
                    native::request(&self.home,"turn/interrupt",json!({"threadId":params["threadId"],"turnId":turn}))
                } else {owner_control::interrupt(&self.home,self.thread(params)?,turn)}
            }
            "desktop/thread/subscribe" => {
                self.thread(params)?;
                self.native_subscribe_page(self.thread(params)?, params["latestOnly"] == true)
            }
            "desktop/file/read" => {
                let thread=self.thread(params)?;
                let meta=native::request(&self.home,"thread/read",json!({"threadId":thread,"includeTurns":false}))?;
                let cwd=meta["thread"]["cwd"].as_str().ok_or("任务目录不可用")?;
                task_files::read(Path::new(cwd),params)
            },
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
            "desktop/notifications/list" => if native::event_ready(&self.home) {
                { let mut result=native::request(&self.home,"clawkit/notifications/read",json!({}))?;
                    notification_metadata::enrich(&self.home,&mut result)?; Ok(result) }
            } else {self.notifications.lock().map_err(|_| "提醒状态不可用")?.read(&self.home)},
            "desktop/status" => Ok(version::status(&self.home)),
            "desktop/thread/statuses" => thread_status::read(&self.home, params),
            "desktop/activity/read" | "desktop/activity/open" | "desktop/activity/clear" | "desktop/activity/close" => {
                #[cfg(target_os="macos")] { renderer::request(&self.home,method.rsplit('/').next().unwrap(),None) }
                #[cfg(not(target_os="macos"))] { Err("此电脑尚不支持官方活动视图同步".into()) }
            },
            "desktop/thread/markRead" => unread::mark_read(&self.home,self.thread(params)?),
            "desktop/thread/search" => self.search.query(&self.home, params["query"].as_str().ok_or("请输入搜索词")?),
            "desktop/thread/list" => catalog_reader::list(&self.home, params),
            "desktop/thread/history" => {
                let thread=self.thread(params)?;
                if let Some(cursor)=params["cursor"].as_str().and_then(|v|v.strip_prefix("engine:")) {
                    self.native_history(thread,cursor)
                } else {
                    Ok(conditional_read::response(saved_history::read_page(&self.home,thread,params["cursor"].as_str(),params["usersOnly"]==true,&self.attachments,if params["latestOnly"]==true {1}else{20})?,params["ifVersion"].as_str()))
                }
            }
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
                if params["latestOnly"] == true { latest_message::compact(&mut visible); }
                Ok(conditional_read::response(json!({"thread":visible}), params["ifVersion"].as_str()))
            }
            "desktop/queue/add" => self.enqueue(params),
            "desktop/queue/cancel" => self.cancel_queue(params),
            "desktop/clarification/answer" => self.answer(params),
            "desktop/turn/start" => if native::event_ready(&self.home) {self.native_start(params)} else {self.start(params)},
            "desktop/thread/create" => self.create(params),
            "desktop/creation/read" => {
                let id = request_id(params)?;
                let project = native::creation::project_key(params)?;
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
                if native::event_ready(&self.home) {
                    if let Ok(result)=self.native_subscribe(thread) {
                        self.native_reconcile(thread,&result["thread"])?;
                    }
                } else if let Ok(mut client) = ipc::IpcClient::connect(&self.home) {
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
            return Err("新建仅接受可选项目和首条消息，模型与权限沿用 Codex 设置".into());
        }
        let id = request_id(params)?;
        let project_id = native::creation::project_key(params)?;
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
            Err("当前 Codex 尚未接入新建通道；启用手机远控后，下次正常启动即可接入".into())
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
