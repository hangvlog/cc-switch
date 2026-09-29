//! Project creation through the user's original desktop window, with durable receipts.
use crate::{catalog, now_ms};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path};

pub struct Creations(Connection);
pub struct Project {
    pub id: String,
    pub legacy_id: String,
    pub name: String,
    pub cwd: String,
}

pub fn project(home: &Path, id: &str) -> Result<Project, String> {
    uuid::Uuid::parse_str(id).map_err(|_| "项目 ID 无效")?;
    let db = catalog::connection(home)?;
    let projects = crate::sidebar::projects(&db)?;
    let p = projects
        .iter()
        .find(|p| p["id"] == id)
        .ok_or("项目已不存在，请刷新项目列表")?;
    let cwd = p["roots"][0]
        .as_str()
        .ok_or("项目没有工作目录，请在电脑补齐项目目录")?;
    if !Path::new(cwd).is_absolute() || !Path::new(cwd).is_dir() {
        return Err("项目目录不存在，请在电脑修复项目路径".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(home.join(".codex-global-state.json"))
        .map_err(|_| "无法读取电脑项目映射")?
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取电脑项目映射")?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("电脑项目映射过大".into());
    }
    let state: Value = serde_json::from_slice(&bytes).map_err(|_| "电脑项目映射格式不兼容")?;
    let host = format!("local:{}", home.display());
    let legacy = state["app-server-project-id-by-legacy-project-id-by-host"][&host]
        .as_object()
        .and_then(|m| m.iter().find(|(_, v)| v.as_str() == Some(id)))
        .map(|(k, _)| k.as_str())
        .unwrap_or(id);
    if state["local-projects"][legacy]["name"] != p["name"] {
        return Err("电脑项目尚未同步，请刷新后再新建".into());
    }
    let native = &state["local-projects"][legacy];
    if native["rootPaths"][0].as_str() != Some(cwd) {
        return Err("电脑项目目录尚未同步，请刷新后重试".into());
    }
    if projects
        .iter()
        .filter(|other| other["name"] == p["name"])
        .count()
        != 1
    {
        return Err("电脑存在同名项目，请先区分项目名称再新建".into());
    }
    Ok(Project {
        id: id.into(),
        legacy_id: legacy.into(),
        name: p["name"].as_str().unwrap_or_default().into(),
        cwd: cwd.into(),
    })
}

impl Creations {
    pub fn open(storage: &Path) -> Result<Self, String> {
        let db = Connection::open(storage.join("remote-dispatches.sqlite"))
            .map_err(|e| e.to_string())?;
        db.busy_timeout(std::time::Duration::from_secs(2))
            .map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS creations(
            request_id TEXT PRIMARY KEY, project_id TEXT NOT NULL,cwd TEXT NOT NULL,text_hash TEXT NOT NULL,
            started_at INTEGER NOT NULL,status TEXT NOT NULL,thread_id TEXT);
            CREATE UNIQUE INDEX IF NOT EXISTS pending_creation ON creations((1)) WHERE status='unknown';").map_err(|e|e.to_string())?;
        Ok(Self(db))
    }
    pub fn not_sent(&self, id: &str) -> bool {
        self.0.query_row("SELECT NOT EXISTS(SELECT 1 FROM creations WHERE request_id=?1 AND status!='rejected')",[id],|r| r.get(0)).unwrap_or(false)
    }
    pub fn begin(&self, id: &str, p: &Project, text: &str) -> Result<(), String> {
        self.0
            .execute(
                "INSERT INTO creations VALUES(?1,?2,?3,?4,?5,'unknown',NULL)",
                params![id, p.id, p.cwd, hash(text), now_ms()],
            )
            .map_err(|_| "还有一次新建结果未确认，请先核对电脑上的会话")?;
        Ok(())
    }
    pub fn reject(&self, id: &str) -> Result<(), String> {
        self.0
            .execute(
                "UPDATE creations SET status='rejected' WHERE request_id=?1",
                [id],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn same(&self, id: &str, project: &str, text: &str) -> Result<bool, String> {
        let old: Option<(String, String)> = self
            .0
            .query_row(
                "SELECT project_id,text_hash FROM creations WHERE request_id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some((p, h)) = old {
            if p != project || h != hash(text) {
                return Err("新建请求编号已用于不同内容".into());
            }
            return Ok(true);
        }
        Ok(false)
    }
    pub fn read(&self, home: &Path, id: &str, project: &str) -> Result<Value, String> {
        let (cwd,digest,start,mut status,mut thread):(String,String,i64,String,Option<String>)=self.0.query_row(
            "SELECT cwd,text_hash,started_at,status,thread_id FROM creations WHERE request_id=?1 AND project_id=?2",params![id,project],
            |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).map_err(|_|"找不到新建回执，请核对电脑；不会自动重发")?;
        if status == "unknown" {
            let db = catalog::connection(home)?;
            let mut q=db.prepare("SELECT id,first_user_message,project_id FROM threads WHERE created_at_ms>=?1 AND created_at_ms<=?1+120000 AND cwd=?2 AND source NOT LIKE '%subAgent%'").map_err(|e|e.to_string())?;
            let rows = q
                .query_map(params![start, cwd], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                })
                .map_err(|e| e.to_string())?;
            let mut matches = vec![];
            for row in rows {
                let (tid, text, pid) = row.map_err(|e| e.to_string())?;
                if hash(&text) == digest {
                    matches.push(json!({"id":tid,"projectId":pid}));
                }
            }
            let projects = crate::sidebar::projects(&db)?;
            crate::sidebar::assign_projects(home, &mut matches, &projects);
            if matches.len() == 1 && matches[0]["projectId"] == project {
                thread = matches[0]["id"].as_str().map(str::to_owned);
                status = "accepted".into();
                self.0
                    .execute(
                        "UPDATE creations SET status='accepted',thread_id=?2 WHERE request_id=?1",
                        params![id, thread],
                    )
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(json!({"requestId":id,"projectId":project,"threadId":thread,"status":status}))
    }
}
fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

#[cfg(target_os = "macos")]
pub fn prepare(home: &Path) -> Result<i32, String> {
    if !crate::version::can_send(home) {
        return Err("当前电脑系统或 Codex 版本尚不支持新建".into());
    }
    let pid = crate::version::running_pid(home).ok_or("Codex 桌面尚未运行")?;
    if crate::accessibility::Window::read(pid)?.has_draft() {
        return Err("电脑当前窗口有未发送草稿，请先处理草稿后再新建；不会覆盖".into());
    }
    Ok(pid)
}
#[cfg(target_os = "macos")]
pub fn submit(
    home: &Path,
    p: &Project,
    text: &str,
    pid: i32,
    enabled: &std::sync::atomic::AtomicBool,
    before_press: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    crate::version::open_new(home, &p.legacy_id, &p.cwd, text)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if !enabled.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("远程连接已关闭，尚未发送".into());
        }
        let window = crate::accessibility::Window::read(pid)?;
        if window.validate(&p.name, text).is_ok() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            return Err("未能确认 Codex 新建页的项目或内容，尚未发送；请查看电脑窗口".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let window = crate::accessibility::Window::read(pid)?;
    window.validate(&p.name, text)?;
    if !enabled.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("远程连接已关闭，尚未发送".into());
    }
    before_press()?;
    window.press_send(&p.name, text)
}
