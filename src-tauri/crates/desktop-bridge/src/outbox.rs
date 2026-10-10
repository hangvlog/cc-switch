use crate::{history, ipc::IpcClient, request_id, Bridge};
use serde_json::{json, Value};
impl Bridge {
    pub(crate) fn enqueue(&self, params: &Value) -> Result<Value, String> {
        if params.as_object().is_none_or(|o| {
            o.keys()
                .any(|k| !["threadId", "requestId", "text"].contains(&k.as_str()))
        }) {
            return Err("队列仅接受消息正文".into());
        }
        let thread = self.thread(params)?;
        let id = request_id(params)?;
        let text = params["text"]
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.len() <= 16000)
            .ok_or("消息不能为空且最多 16000 字节")?;
        if let Some(old) = self.queue.existing(id, thread, text)? {
            return Ok(old);
        }
        if !self.enabled.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("远程连接已关闭".into());
        }
        let snap = IpcClient::passive_snapshot(&self.home, thread)?;
        if let Some(reason) = history::conversation_block_reason(&snap.state) {
            return Err(reason.into());
        }
        if self.ledger.has_request(id)? {
            return Err("请求编号已用于直接发送".into());
        }
        self.queue.add(id, thread, text)
    }
    pub(crate) fn cancel_queue(&self, params: &Value) -> Result<Value, String> {
        let thread = self.thread(params)?;
        let id = request_id(params)?;
        if self.ledger.has_request(id)? {
            let status = self.ledger.read(id, thread)?;
            self.queue
                .mark(id, status["status"].as_str().unwrap_or("unknown"), None)?;
        }
        self.queue.cancel(id, thread)
    }
    /// Invoked by the desktop runtime even when the phone is disconnected/backgrounded.
    pub fn drain_queue(&self) -> Result<(), String> {
        if !self.enabled.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }
        for (id, thread, text) in self.queue.heads()? {
            let Ok(snap) = IpcClient::passive_snapshot(&self.home, &thread) else {
                continue;
            };
            self.reconcile(&thread, &snap.state)?;
            if self.ledger.has_request(&id)? {
                let receipt = self.ledger.read(&id, &thread)?;
                let status = receipt["status"].as_str().unwrap_or("unknown");
                self.queue.mark(
                    &id,
                    status,
                    if status == "unknown" {
                        Some("发送结果待核对，不会自动重发")
                    } else if status == "rejected" {
                        Some("发送未成功，请取消后重新发送")
                    } else {
                        None
                    },
                )?;
                continue;
            }
            // Pending clarifications/approvals, errors and active turns keep the outbox paused.
            if !crate::clarification::pending(&snap.state).is_empty()
                || history::send_block_reason(&snap.state).is_some()
                || snap.state["threadRuntimeStatus"]["type"] != "idle"
                || history::turns(&snap.state)
                    .last()
                    .is_some_and(|t| t["status"] != "completed")
            {
                continue;
            }
            match self.start(&json!({"threadId":thread,"requestId":id,"text":text})) {
                Ok(r) => self
                    .queue
                    .mark(&id, r["status"].as_str().unwrap_or("unknown"), None)?,
                Err(_) => {
                    if self.ledger.has_request(&id)? {
                        self.queue
                            .mark(&id, "unknown", Some("发送结果待核对，不会自动重发"))?;
                    }
                }
            }
        }
        Ok(())
    }
}
