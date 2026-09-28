use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::path::Path;

fn connection(home: &Path) -> Result<Connection, String> {
    let database = home.join("state_5.sqlite");
    let connection = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "Codex 任务索引尚不存在或版本不兼容")?;
    connection
        .busy_timeout(std::time::Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    connection
        .pragma_update(None, "query_only", true)
        .map_err(|e| e.to_string())?;
    Ok(connection)
}

pub fn list(home: &Path, offset: usize) -> Result<Value, String> {
    let connection = connection(home)?;
    let mut query = connection.prepare("SELECT id,title,cwd,created_at,updated_at FROM threads WHERE archived=0 AND source NOT LIKE '%subAgent%' ORDER BY updated_at DESC,id LIMIT 51 OFFSET ?1")
        .map_err(|_| "Codex 任务索引结构已变化")?;
    let rows = query.query_map([offset as i64],|row| Ok(json!({"id":row.get::<_,String>(0)?,
        "preview":row.get::<_,String>(1)?,"cwd":row.get::<_,String>(2)?,"createdAt":row.get::<_,i64>(3)?,
        "updatedAt":row.get::<_,i64>(4)?,"turns":[],"desktop":{"source":"catalog","status":"unknown","canSend":false}})))
        .map_err(|e| e.to_string())?;
    let mut data = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let next = if data.len() > 50 {
        data.truncate(50);
        Some((offset + 50).to_string())
    } else {
        None
    };
    Ok(json!({"data":data,"nextCursor":next}))
}

pub fn verify_thread(home: &Path, thread: &str) -> Result<(), String> {
    uuid::Uuid::parse_str(thread).map_err(|_| "任务 ID 无效")?;
    let connection = connection(home)?;
    let eligible: bool = connection
        .query_row(
            "SELECT archived=0 AND source NOT LIKE '%subAgent%' FROM threads WHERE id=?1",
            [thread],
            |row| row.get(0),
        )
        .map_err(|_| "原任务不存在")?;
    if !eligible {
        return Err("已归档或子代理任务不能远程操作".into());
    }
    Ok(())
}
