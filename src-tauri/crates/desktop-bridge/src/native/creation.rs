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
        if !project.is_empty() {
            uuid::Uuid::parse_str(project).map_err(|_| "项目 ID 无效")?;
        }
        if !enabled.load(Ordering::SeqCst) {
            return Err("远程连接已关闭，尚未发送".into());
        }
        if !super::ready(home) {
            return Err("当前 Codex 尚未接入新建通道；启用手机远控后，下次正常启动 Codex 即可接入，无需中断当前任务".into());
        }
        let cwd = if project.is_empty() {
            None
        } else {
            let p = super::request(home, "project/read", json!({"projectId":project}))?;
            if p["project"]["id"] != project {
                return Err("电脑项目未同步，尚未发送".into());
            }
            let cwd = p["project"]["roots"][0]["path"]
                .as_str()
                .ok_or("项目没有工作目录")?
                .to_owned();
            if !Path::new(&cwd).is_absolute() || !Path::new(&cwd).is_dir() {
                return Err("项目工作目录不存在".into());
            }
            Some(cwd)
        };
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
        let cwd = match cwd
            .map(Ok)
            .unwrap_or_else(|| self.projectless_directory(id))
        {
            Ok(cwd) => cwd,
            Err(error) => {
                self.mark(id, "rejected")?;
                return Err(error);
            }
        };
        let project_value = if project.is_empty() {
            Value::Null
        } else {
            json!(project)
        };
        // Once the request enters the transport, failures remain unknown and are not retried.
        let started = super::request(
            home,
            "thread/start",
            json!({"projectId":project_value,"cwd":cwd}),
        );
        if let Ok(started) = started {
            if let Some(thread) = started["thread"]["id"].as_str().filter(|_| {
                matches_project(&started["thread"], project)
                    && started["thread"]["cwd"].as_str().is_some_and(|returned| {
                        Path::new(returned).canonicalize().ok()
                            == Path::new(&cwd).canonicalize().ok()
                    })
            }) {
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
    fn projectless_directory(&self, id: &str) -> Result<String, String> {
        // Match Codex's projectless workspace convention, never inherit an engine's
        // process cwd (which can be a previous project or the user's home).
        uuid::Uuid::parse_str(id).map_err(|_| "新建请求编号无效")?;
        let user = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_absolute())
            .ok_or("找不到电脑用户目录")?;
        let date: String = self
            .0
            .query_row("SELECT strftime('%Y-%m-%d','now','localtime')", [], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())?;
        let parent = user.join("Documents").join("Codex").join(date);
        std::fs::create_dir_all(&parent).map_err(|e| format!("无法创建无项目工作目录：{e}"))?;
        let directory = parent.join(format!("chat-{id}"));
        std::fs::create_dir(&directory).map_err(|e| format!("无法创建无项目工作目录：{e}"))?;
        Ok(directory.to_string_lossy().into_owned())
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
                    let confirmed = matches_project(&value["thread"], project)
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
            json!({"requestId":id,"projectId":if project.is_empty() {Value::Null} else {json!(project)},"threadId":thread,"status":status,"via":"native-engine"}),
        )
    }
}

// An empty durable key represents explicit no-project, distinct from every UUID.
pub(crate) fn project_key(params: &Value) -> Result<&str, String> {
    match &params["projectId"] {
        Value::Null => Ok(""),
        Value::String(id) if uuid::Uuid::parse_str(id).is_ok() => Ok(id),
        _ => Err("项目 ID 无效".into()),
    }
}
fn matches_project(thread: &Value, project: &str) -> bool {
    if project.is_empty() {
        thread["projectId"].is_null()
    } else {
        thread["projectId"] == project
    }
}
