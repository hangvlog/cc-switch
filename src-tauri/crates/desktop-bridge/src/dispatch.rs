use crate::{attachments, history, ipc, request_id, version, Bridge};
use serde_json::Value;

impl Bridge {
    pub(crate) fn start(&self, params: &Value) -> Result<Value, String> {
        // Reject settings overrides: the original owner's settings are authoritative.
        let object = params.as_object().ok_or("发送参数无效")?;
        if object.keys().any(|key| {
            !["threadId", "requestId", "text", "images", "files"].contains(&key.as_str())
        }) {
            return Err("续聊不能覆盖原任务的模型、权限或工作目录".into());
        }
        let thread = self.thread(params)?;
        let id = request_id(params)?;
        let message = attachments::Message::parse(params)?;
        let text = &message.identity();
        // Duplicates return their durable receipt even while the desktop is offline/busy.
        if let Some(receipt) = self.ledger.existing(id, thread, text)? {
            return Ok(receipt);
        }
        let input = self.attachments.input(&message)?;
        if !version::can_send(&self.home) {
            return Err("Codex 桌面连接尚未就绪，请重新连接".into());
        }
        let mut client = ipc::IpcClient::connect(&self.home)?;
        let initial = client.snapshot(thread, None)?;
        self.reconcile(thread, &initial.state)?;
        if let Some(reason) = history::dispatch_block_reason(&initial.state) {
            return Err(reason.into());
        }
        let checked = client.snapshot(thread, Some(&initial.owner))?;
        if let Some(reason) = history::dispatch_block_reason(&checked.state) {
            return Err(reason.into());
        }
        // A fresh follower subscription increments revision even when state is identical.
        if !history::can_steer(&checked.state) && checked.state != initial.state {
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
        let steering = history::can_steer(&checked.state);
        let result = if steering {
            client.steer_turn(
                thread,
                &checked.owner,
                id,
                &checked.state["cwd"],
                &message.text,
                input,
            )
        } else {
            client.start_turn(thread, &checked.owner, id, input)
        };
        let status = match result {
            Ok(response)
                if (response["result"]["result"]["turn"].is_object()
                    || response["result"]["turn"].is_object()
                    || steering
                        && response["result"]["result"]["turnId"]
                            .as_str()
                            .is_some_and(|id| !id.is_empty())) =>
            {
                "accepted"
            }
            Err(ref error) if error == "Codex IPC: request-version-mismatch" => "rejected",
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
