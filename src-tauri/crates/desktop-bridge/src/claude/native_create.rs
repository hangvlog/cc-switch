//! Initialize the official CLI using a local-only command, close it, and then
//! deliver the first real message to the sole writer owned by Claude Desktop.
use super::{gateway, inbox, index, message, native_approval, runner, Claude};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::{
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn seed(launch: runner::Launch, sid: &str, title: &str) -> Result<(), String> {
    let mut process = Command::new(launch.cli)
        .args(launch.args)
        .args(["--name", title])
        .current_dir(launch.cwd)
        .env_clear()
        .envs(launch.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "无法初始化 Claude 会话")?;
    let deadline = Instant::now() + Duration::from_secs(7);
    let result = (|| {
        let mut stdin = process.stdin.take().ok_or("无法初始化 Claude 会话")?;
        writeln!(stdin,"{}",json!({"type":"user","uuid":uuid::Uuid::new_v4().to_string(),"session_id":sid,"parent_tool_use_id":null,
            "message":{"role":"user","content":format!("/rename {title}")}})).map_err(|_|"Claude 未收到会话名")?;
        drop(stdin);
        while Instant::now() < deadline {
            if let Some(status) = process
                .try_wait()
                .map_err(|_| "无法核对 Claude 初始化状态")?
            {
                return if status.success() {
                    Ok(())
                } else {
                    Err("Claude 未完成会话初始化".into())
                };
            }
            std::thread::sleep(Duration::from_millis(60));
        }
        Err("Claude 初始化会话超时".into())
    })();
    if result.is_err() {
        let _ = process.kill();
        let _ = process.wait();
    }
    result
}

impl Claude {
    pub(super) fn create_native(
        &self,
        id: &str,
        cwd: &str,
        msg: &message::Message,
        gateway: &gateway::Gateway,
        model: &str,
    ) -> Result<Value, String> {
        let _guard = self.create_lock.lock().map_err(|_| "会话创建状态不可用")?;
        if let Some(r) = self.turns.receipt(id) {
            return Ok(r);
        }
        // The UUID from the phone is also the persistent idempotency key.
        let sid = id;
        if index::find_transcript(&self.paths, sid, cwd).is_some()
            && !native_approval::owns(&self.paths, sid, cwd)
        {
            return Err("此请求编号已被其他 Claude 会话使用".into());
        }
        native_approval::prepare(&self.paths, cwd, sid)?;
        let db = rusqlite::Connection::open(native_approval::database(&self.paths))
            .map_err(|_| "无法检查会话回执")?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS deliveries(session TEXT PRIMARY KEY,text_digest TEXT NOT NULL,sent INTEGER NOT NULL DEFAULT 0);").map_err(|_|"无法检查发送记录")?;
        use sha2::{Digest, Sha256};
        let digest = format!("{:x}", Sha256::digest(msg.content().to_string().as_bytes()));
        db.execute(
            "INSERT OR IGNORE INTO deliveries(session,text_digest) VALUES(?,?)",
            params![sid, digest],
        )
        .map_err(|_| "无法登记发送请求")?;
        let (prior, sent): (String, bool) = db
            .query_row(
                "SELECT text_digest,sent FROM deliveries WHERE session=?",
                [sid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .ok()
            .flatten()
            .ok_or("无法检查发送回执")?;
        if prior != digest {
            return Err("此发送编号已用于另一条消息".into());
        }
        let thread = format!("local_{sid}");
        let receipt = |recorded: bool| json!({"threadId":thread,"turnId":id,"accepted":true,"deliveredTo":"desktop","recorded":recorded});
        let existing = index::find_transcript(&self.paths, sid, cwd);
        if sent {
            return if existing.as_deref().is_some_and(|f| inbox::recorded(f, id)) {
                Ok(receipt(true))
            } else {
                Err("上次发送尚未在 Claude 桌面记录，打开对应会话核对后再发送".into())
            };
        }
        if existing.is_none() {
            let title: String = msg
                .text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .filter(|c| !c.is_control())
                .take(60)
                .collect();
            let title = if title.is_empty() { "图片" } else { &title };
            let launch = self.launch(
                gateway,
                cwd,
                ["--session-id", sid],
                model,
                None,
                None,
            )?;
            seed(launch, sid, title)?;
        }
        (self.adopter)(sid)?;
        let end = Instant::now() + Duration::from_secs(12);
        // A stale PID or an SDK session must never receive the first message.
        let (live, session) = loop {
            let live = index::live_processes(&self.paths)
                .remove(sid)
                .filter(|l| l.host.as_deref() == Some(&thread) && l.socket.is_some());
            if let Some(live) = live {
                if let Ok(session) = index::resolve(&self.paths, &thread) {
                    break (live, session);
                }
            }
            if Instant::now() >= end {
                return Err("Claude 桌面未打开新会话，首条消息尚未发送".into());
            }
            std::thread::sleep(Duration::from_millis(150));
        };
        if session.cwd != cwd || session.archived {
            return Err("Claude 桌面打开的项目或会话状态与手机请求不同".into());
        }
        if session
            .model
            .as_deref()
            .is_some_and(|actual| actual != model)
        {
            return Err("Claude 桌面采用了不同的模型，首条消息尚未发送".into());
        }
        #[cfg(target_os = "macos")]
        if super::permission_mode(&self.paths, session.permission_mode.as_deref()) == "bypassPermissions" {
            if let Some(reason) = inbox::blocked_reason(inbox::inbound(&self.paths, cwd, Some("bypassPermissions"))) {
                return Err(reason.into());
            }
            super::native_mode::ensure(&self.paths, &session)?;
        }
        let transcript = session.transcript.ok_or("Claude 桌面没有新会话的记录")?;
        if let Some(reason) = inbox::blocked_reason(inbox::inbound(
            &self.paths,
            cwd,
            session.permission_mode.as_deref(),
        )) {
            return Err(reason.into());
        }
        let text = msg.peer_text(id)?;
        let changed = db
            .execute(
                "UPDATE deliveries SET sent=1 WHERE session=? AND sent=0",
                [sid],
            )
            .map_err(|_| "无法确认发送状态")?;
        if changed != 1 {
            return Err("同一条消息正在发送".into());
        }
        inbox::deliver(&self.paths, &live, id, &text)?;
        for _ in 0..24 {
            if inbox::recorded(&transcript, id) {
                let ok = receipt(true);
                self.turns.remember(id, &ok);
                return Ok(ok);
            }
            std::thread::sleep(Duration::from_millis(120))
        }
        Err("Claude 桌面已打开，尚未确认首条消息送达，请查看会话".into())
    }
}
