//! A Claude PermissionRequest hook for sessions created by this paired account.
//! Claude still owns the prompt. A local decision cancels its hook process.
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    fs,
    io::Read,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const FLAG: &str = "--clawkit-claude-permission";
const LIMIT: u64 = 2 * 1024 * 1024;
const TTL: u64 = 3600;
fn open_db(file: &Path) -> Result<Connection, String> {
    let db = Connection::open_with_flags(file, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|_| "无法读取手机审批状态")?;
    db.busy_timeout(Duration::from_secs(2))
        .map_err(|_| "手机审批状态繁忙")?;
    Ok(db)
}

pub fn database(paths: &super::Paths) -> PathBuf {
    paths.registry.with_file_name("claude-native.sqlite")
}

pub fn owns(paths: &super::Paths, sid: &str, cwd: &str) -> bool {
    open_db(&database(paths))
        .ok()
        .and_then(|db| {
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM owners WHERE session=? AND cwd=?)",
                params![sid, cwd],
                |r| r.get(0),
            )
            .ok()
        })
        .unwrap_or(false)
}

pub fn prepare(paths: &super::Paths, cwd: &str, session: &str) -> Result<(), String> {
    let file = database(paths);
    fs::create_dir_all(file.parent().ok_or("缺少账户目录")?).map_err(|_| "无法创建账户目录")?;
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&file)
        .map_err(|_| "无法创建手机审批状态")?;
    let db = open_db(&file)?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS owners(session TEXT PRIMARY KEY,cwd TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS prompts(id TEXT PRIMARY KEY,session TEXT NOT NULL,pid INTEGER NOT NULL,created INTEGER NOT NULL,payload TEXT NOT NULL,decision TEXT);").map_err(|_| "无法初始化手机审批状态")?;
    install_hook(Path::new(cwd), &file)?;
    db.execute(
        "INSERT OR IGNORE INTO owners(session,cwd) VALUES(?1,?2)",
        params![session, cwd],
    )
    .map_err(|_| "无法登记手机会话")?;
    db.execute("DELETE FROM prompts WHERE created < unixepoch()-3600", [])
        .ok();
    Ok(())
}

fn shell_arg(path: &Path) -> Result<String, String> {
    let s = path
        .to_str()
        .filter(|p| !p.chars().any(char::is_control))
        .ok_or("Claude hook 路径无效")?;
    Ok(format!("'{}'", s.replace('\'', "'\\''")))
}

fn install_hook(cwd: &Path, db: &Path) -> Result<(), String> {
    use std::io::Write;
    let dir = cwd.join(".claude");
    fs::create_dir_all(&dir).map_err(|_| "无法打开项目的 Claude 设置目录")?;
    if !fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
        return Err("Claude 项目设置目录指向其他位置".into());
    }
    let path = dir.join("settings.local.json");
    let original = match fs::read(&path) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err("无法读取项目 Claude 本地设置".into()),
    };
    let mut settings: Value = match &original {
        Some(b) => serde_json::from_slice(b).map_err(|_| "项目 Claude 本地设置不是有效 JSON")?,
        None => json!({}),
    };
    if settings["disableAllHooks"] == true || settings["allowManagedHooksOnly"] == true {
        return Err("项目 Claude 设置已停用本地审批 hook".into());
    }
    let command = format!(
        "{} {} {}",
        shell_arg(&std::env::current_exe().map_err(|_| "找不到 ClawKit 可执行文件")?)?,
        FLAG,
        shell_arg(db)?
    );
    let hooks = settings
        .as_object_mut()
        .ok_or("Claude 设置格式无效")?
        .entry("hooks")
        .or_insert_with(|| json!({}));
    let groups = hooks
        .as_object_mut()
        .ok_or("Claude hooks 格式无效")?
        .entry("PermissionRequest")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or("PermissionRequest 格式无效")?;
    if groups.iter().any(|g| {
        g["hooks"].as_array().is_some_and(|a| {
            a.iter()
                .any(|h| h["type"] == "command" && h["command"] == command)
        })
    }) {
        return Ok(());
    }
    groups.push(json!({"hooks":[{"type":"command","command":command,"timeout":TTL}]}));
    let tmp = dir.join(format!(".clawkit-{}.tmp", uuid::Uuid::new_v4()));
    let write = || -> Result<(), String> {
        let mut f = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|_| "无法暂存 Claude 设置")?;
        serde_json::to_writer_pretty(&mut f, &settings).map_err(|_| "无法保存 Claude 设置")?;
        f.write_all(b"\n")
            .and_then(|_| f.sync_all())
            .map_err(|_| "无法写入 Claude 设置")?;
        if let Ok(meta) = fs::symlink_metadata(&path) {
            if !meta.is_file() {
                return Err("项目 Claude 设置指向其他文件，无法安全合并".into());
            }
            fs::set_permissions(&tmp, fs::Permissions::from_mode(meta.permissions().mode()))
                .map_err(|_| "无法保留 Claude 设置权限")?;
        }
        if fs::read(&path).ok() != original {
            return Err("项目 Claude 设置刚发生变化，请重试".into());
        }
        fs::rename(&tmp, &path).map_err(|_| "无法更新项目 Claude 设置".into())
    };
    let result = write();
    let _ = fs::remove_file(tmp);
    result
}

fn alive(pid: i64) -> bool {
    pid > 1 && pid <= i32::MAX as i64 && unsafe { libc::kill(pid as i32, 0) == 0 }
}
pub fn pending(file: &Path, sid: &str) -> Vec<Value> {
    let Ok(db) = open_db(file) else { return vec![] };
    let Ok(mut stmt)=db.prepare("SELECT pid,payload FROM prompts WHERE session=?1 AND decision IS NULL AND created>=unixepoch()-3600 ORDER BY created,id") else{return vec![]};
    let Ok(rows) = stmt.query_map([sid], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
    else {
        return vec![];
    };
    rows.flatten()
        .filter(|(pid, _)| alive(*pid))
        .filter_map(|(_, s)| serde_json::from_str(&s).ok())
        .collect()
}

pub fn respond(
    file: &Path,
    sid: &str,
    id: &str,
    allow: bool,
    answers: &Value,
) -> Result<Value, String> {
    let request = pending(file, sid)
        .into_iter()
        .find(|r| r["requestId"] == id)
        .ok_or("这条审批已结束，请刷新会话")?;
    let result = if allow {
        if request["kind"] == "question" {
            json!({"behavior":"allow","updatedInput":super::runner::answered_input(&request["input"],answers)})
        } else {
            json!({"behavior":"allow"})
        }
    } else {
        json!({"behavior":"deny","message":"已在 ClawKit 手机端拒绝"})
    };
    let db = open_db(file)?;
    let n = db
        .execute(
            "UPDATE prompts SET decision=?1 WHERE session=?2 AND id=?3 AND decision IS NULL",
            params![result.to_string(), sid, id],
        )
        .map_err(|_| "无法提交审批")?;
    if n != 1 {
        return Err("这条审批已处理".into());
    }
    Ok(json!({"accepted":true}))
}

pub fn maybe_run_hook() {
    if std::env::args_os().nth(1).as_deref() != Some(std::ffi::OsStr::new(FLAG)) {
        return;
    }
    if let Some(file) = std::env::args_os().nth(2) {
        if let Ok(Some(decision)) = run_hook(Path::new(&file)) {
            println!(
                "{}",
                json!({"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":decision}})
            );
        }
    }
    std::process::exit(0);
}

fn run_hook(file: &Path) -> Result<Option<Value>, String> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "stdin")?;
    if bytes.len() as u64 > LIMIT {
        return Ok(None);
    }
    let event: Value = serde_json::from_slice(&bytes).map_err(|_| "json")?;
    let sid = event["session_id"]
        .as_str()
        .filter(|s| super::index::is_uuid(s))
        .ok_or("session")?;
    if event["hook_event_name"] != "PermissionRequest" {
        return Ok(None);
    }
    let db = open_db(file)?;
    let cwd = event["cwd"].as_str().ok_or("cwd")?;
    let owns: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM owners WHERE session=?1 AND cwd=?2)",
            params![sid, cwd],
            |r| r.get(0),
        )
        .map_err(|_| "owner")?;
    if !owns {
        return Ok(None);
    }
    let id = format!("native:{}", uuid::Uuid::new_v4());
    let tool = event["tool_name"].as_str().ok_or("tool")?;
    let payload = json!({"requestId":id,"tool":tool,"title":tool,"input":event["tool_input"],"kind":if tool=="AskUserQuestion"{"question"}else{"approval"}});
    db.execute(
        "INSERT INTO prompts(id,session,pid,created,payload) VALUES(?,?,?,unixepoch(),?)",
        params![id, sid, std::process::id(), payload.to_string()],
    )
    .map_err(|_| "insert")?;
    let start = Instant::now();
    let mut result = None;
    while start.elapsed() < Duration::from_secs(TTL - 2) {
        let value: Option<Option<String>> = db
            .query_row("SELECT decision FROM prompts WHERE id=?", [&id], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|_| "decision")?;
        match value {
            Some(Some(s)) => {
                result = serde_json::from_str(&s).ok();
                break;
            }
            None => break,
            _ => std::thread::sleep(Duration::from_millis(250)),
        }
    }
    let _ = db.execute("DELETE FROM prompts WHERE id=?", [&id]);
    Ok(result)
}
