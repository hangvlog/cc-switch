//! Durable native creation receipts: never infer acceptance from matching text.
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
pub(crate) struct NativeCreations(Connection);
impl NativeCreations {
    pub fn open(storage: &Path) -> Result<Self, String> {
        // Reuse private durable storage initialization even for a new account scope.
        let _ = crate::ledger::Ledger::open(storage)?;
        let db = Connection::open(storage.join("remote-dispatches.sqlite"))
            .map_err(|e| e.to_string())?;
        db.busy_timeout(std::time::Duration::from_secs(2))
            .map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA synchronous=FULL;
          CREATE TABLE IF NOT EXISTS native_creations(request_id TEXT PRIMARY KEY,project_id TEXT NOT NULL,text_hash TEXT NOT NULL,thread_id TEXT,status TEXT NOT NULL);
          CREATE UNIQUE INDEX IF NOT EXISTS pending_native_creation ON native_creations((1)) WHERE status='unknown';").map_err(|e|e.to_string())?;
        Ok(Self(db))
    }
    pub fn exists(&self, id: &str) -> bool {
        self.0
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM native_creations WHERE request_id=?1)",
                [id],
                |r| r.get(0),
            )
            .unwrap_or(false)
    }
    pub fn not_sent(&self, id: &str) -> bool {
        self.0.query_row("SELECT NOT EXISTS(SELECT 1 FROM native_creations WHERE request_id=?1 AND status!='rejected')",[id],|r|r.get(0)).unwrap_or(false)
    }
    pub fn create(
        &self,
        home: &Path,
        id: &str,
        project: &str,
        text: &str,
        input: Value,
        enabled: &AtomicBool,
    ) -> Result<Value, String> {
        let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
        let existing: Option<(String, String)> = self
            .0
            .query_row(
                "SELECT project_id,text_hash FROM native_creations WHERE request_id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some((p, h)) = existing {
            if p != project || h != digest {
                return Err("新建请求编号已用于不同内容".into());
            }
            return self.read(home, id, project);
        }
        uuid::Uuid::parse_str(project).map_err(|_| "项目 ID 无效")?;
        if !enabled.load(Ordering::SeqCst) {
            return Err("远程连接已关闭，尚未发送".into());
        }
        if !super::ready(home) {
            return Err("请先从 ClawKit 打开 Codex，启用无需辅助功能的新建通道".into());
        }
        let p = super::request(home, "project/read", json!({"projectId":project}))?;
        if p["project"]["id"] != project {
            return Err("电脑项目未同步，尚未发送".into());
        }
        let cwd = p["project"]["roots"][0]["path"]
            .as_str()
            .ok_or("项目没有工作目录")?;
        if !Path::new(cwd).is_absolute() || !Path::new(cwd).is_dir() {
            return Err("项目工作目录不存在".into());
        }
        self.0
            .execute(
                "INSERT INTO native_creations VALUES(?1,?2,?3,NULL,'unknown')",
                params![id, project, digest],
            )
            .map_err(|_| "上次新建结果尚未确认，请先查看回执")?;
        if !enabled.load(Ordering::SeqCst) {
            self.mark(id, "rejected")?;
            return self.read(home, id, project);
        }
        // Once the request enters the transport, failures remain unknown and are not retried.
        let started = super::request(home, "thread/start", json!({"projectId":project,"cwd":cwd}));
        if let Ok(started) = started {
            if let Some(thread) = started["thread"]["id"]
                .as_str()
                .filter(|_| started["thread"]["projectId"] == project)
            {
                uuid::Uuid::parse_str(thread).map_err(|_| "Codex 返回了无效对话编号")?;
                self.0
                    .execute(
                        "UPDATE native_creations SET thread_id=?2 WHERE request_id=?1",
                        params![id, thread],
                    )
                    .map_err(|e| e.to_string())?;
                if enabled.load(Ordering::SeqCst) {
                    if let Ok(turn) = super::request(
                        home,
                        "turn/start",
                        json!({"threadId":thread,"clientUserMessageId":id,"input":input}),
                    ) {
                        if turn["turn"]["id"].is_string() {
                            self.mark(id, "accepted")?;
                        }
                    }
                }
            }
        }
        self.read(home, id, project)
    }
    fn mark(&self, id: &str, status: &str) -> Result<(), String> {
        self.0
            .execute(
                "UPDATE native_creations SET status=?2 WHERE request_id=?1",
                params![id, status],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn read(&self, home: &Path, id: &str, project: &str) -> Result<Value, String> {
        let(thread,mut status):(Option<String>,String)=self.0.query_row("SELECT thread_id,status FROM native_creations WHERE request_id=?1 AND project_id=?2",params![id,project],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|"找不到此项目的新建回执；不会自动重发")?;
        if status == "unknown" {
            if let Some(thread) = &thread {
                if let Ok(value) = super::request(
                    home,
                    "thread/read",
                    json!({"threadId":thread,"includeTurns":true}),
                ) {
                    let confirmed = value["thread"]["projectId"] == project
                        && value["thread"]["turns"].as_array().is_some_and(|turns| {
                            turns.iter().any(|t| {
                                t["items"].as_array().is_some_and(|items| {
                                    items
                                        .iter()
                                        .any(|i| i["type"] == "userMessage" && i["clientId"] == id)
                                })
                            })
                        });
                    if confirmed {
                        self.mark(id, "accepted")?;
                        status = "accepted".into();
                    }
                }
            }
        }
        Ok(
            json!({"requestId":id,"projectId":project,"threadId":thread,"status":status,"via":"native-engine"}),
        )
    }
}
