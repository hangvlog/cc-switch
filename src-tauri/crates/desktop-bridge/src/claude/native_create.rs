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
        // The UUID from the phone is also the persistent idempotency key.
        let sid = id;
        let existing = index::find_transcript(&self.paths, sid, cwd);
        let has_state = existing.is_some()
            || index::creation_writer(&self.paths, sid)?.is_some()
            || index::imported_session(&self.paths, sid)?.is_some();
        let owned = native_approval::owns(&self.paths, sid, cwd);
        if has_state && !owned {
            return Err("此请求编号已被其他 Claude 会话使用".into());
        }
        if !owned {
            native_approval::prepare(&self.paths, cwd, sid)?;
        }
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
        if sent {
            if let Some(r) = self.turns.receipt(id).filter(|r| r["threadId"] == thread && (r["recorded"] == true || r["deliveredTo"].is_null())) {
                return Ok(r);
            }
            return if existing.as_deref().is_some_and(|f| inbox::recorded(f, id)) {
                Ok(receipt(true))
            } else {
                Err("上次发送尚未在 Claude 桌面记录，打开对应会话核对后再发送".into())
            };
        }
        if owned {
            native_approval::prepare(&self.paths, cwd, sid)?;
        }
        if !has_state {
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
        let mut may_open = true;
        #[cfg(target_os = "macos")]
        let mut ui_guard: Option<super::native_mode::UiGuard> = None;
        let mut opened_here = false;
        let mut session_seen_at: Option<Instant> = None;
        let end = Instant::now() + Duration::from_secs(12);
        let (mut live, session) = loop {
            let session = index::imported_session(&self.paths, sid)?;
            if session.as_ref().is_some_and(|s| s.cwd != cwd || s.archived) {
                return Err("Claude 桌面的会话与手机请求不匹配，首条消息尚未发送".into());
            }
            if session.is_some() && session_seen_at.is_none() {
                session_seen_at = Some(Instant::now());
            }
            match index::creation_writer(&self.paths, sid)? {
                Some(writer) => {
                    // Desktop can publish its process before its index or inbox.
                    // Once observed, never navigate to replace it on this attempt.
                    may_open = false;
                    if writer.host.as_deref() != Some(&thread) {
                        return Err("此会话正由其他 Claude 进程持有，首条消息尚未发送".into());
                    }
                    if let Some(session) = session {
                        if writer.socket.is_some() {
                            break (Some(writer), session);
                        }
                    }
                }
                None if may_open => {
                    #[cfg(target_os = "macos")]
                    {
                        let mut guard = super::native_mode::UiGuard::capture();
                        (self.adopter)(sid)?;
                        guard.mark_navigated(thread.clone());
                        ui_guard = Some(guard);
                    }
                    #[cfg(not(target_os = "macos"))]
                    (self.adopter)(sid)?;
                    may_open = false;
                    opened_here = true;
                }
                None => {
                    // When Claude Desktop's CliGovernor is at its warm-process cap,
                    // import succeeds and registers the sidebar row without warming
                    // a live CLI writer. After a brief grace window, proceed with
                    // the registered native session and fall back to CLI resume.
                    if let Some(session) = session {
                        if session_seen_at.is_some_and(|t| t.elapsed() >= Duration::from_millis(2500)) {
                            break (None, session);
                        }
                    }
                }
            }
            if Instant::now() >= end {
                return Err("Claude 桌面会话尚未就绪，首条消息尚未发送".into());
            }
            std::thread::sleep(Duration::from_millis(150));
        };
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
            super::native_mode::ensure_in_place(&self.paths, &session, opened_here)?;
        }
        #[cfg(target_os = "macos")]
        if let Some(mut guard) = ui_guard.take() {
            guard.restore();
        }
        if live.is_none() {
            if let Some(writer) = index::creation_writer(&self.paths, sid)? {
                if writer.host.as_deref() != Some(&thread) {
                    return Err("此会话正由其他 Claude 进程持有，首条消息尚未发送".into());
                }
                if writer.socket.is_some() {
                    live = Some(writer);
                }
            }
        }
        let transcript = session.transcript.ok_or("Claude 桌面没有新会话的记录")?;
        if let Some(live) = live {
            if let Some(reason) = inbox::blocked_reason(inbox::inbound(
                &self.paths,
                cwd,
                session.permission_mode.as_deref(),
            )) {
                return Err(reason.into());
            }
            let text = msg.peer_text(id)?;
            // A stale socket did not receive any bytes. Leave this request retryable
            // until connected, then claim it before writing to prevent duplicates.
            let stream = inbox::connect(&live)?;
            let changed = db
                .execute(
                    "UPDATE deliveries SET sent=1 WHERE session=? AND sent=0",
                    [sid],
                )
                .map_err(|_| "无法确认发送状态")?;
            if changed != 1 {
                return Err("同一条消息正在发送".into());
            }
            inbox::send_connected(stream, &self.paths, &live, id, &text)?;
            for _ in 0..24 {
                if inbox::recorded(&transcript, id) {
                    let ok = receipt(true);
                    self.turns.remember(id, &ok);
                    return Ok(ok);
                }
                std::thread::sleep(Duration::from_millis(120))
            }
            return Err("Claude 桌面已打开，尚未确认首条消息送达，请查看会话".into());
        }
        let launch = self.launch(
            gateway,
            cwd,
            ["--resume", &session.cli_id],
            model,
            session.permission_mode.as_deref(),
            session.cli_version.as_deref(),
        )?;
        let changed = db
            .execute(
                "UPDATE deliveries SET sent=1 WHERE session=? AND sent=0",
                [sid],
            )
            .map_err(|_| "无法确认发送状态")?;
        if changed != 1 {
            return Err("同一条消息正在发送".into());
        }
        let started = self.turns.start(
            &self.events,
            &session.cli_id,
            &thread,
            id,
            &msg.content(),
            launch,
            None,
        );
        if started.is_err() {
            let _ = db.execute("UPDATE deliveries SET sent=0 WHERE session=?", [sid]);
        }
        started
    }
}
