//! Bounded, read-only pages from the original Codex transcript. No IPC or navigation.
use crate::catalog;
use rusqlite::params;
use serde_json::{json, Value};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};
const WINDOW: u64 = 4 * 1024 * 1024;

fn visible(event: &Value, thread: &str, offset: u64, end: u64) -> Option<Value> {
    if event["type"] != "event_msg" {
        return None;
    }
    let p = &event["payload"];
    let (kind, id, content, phase) = if p["type"] == "item_completed" {
        if p["thread_id"].as_str().is_some_and(|id| id != thread) {
            return None;
        }
        let item = &p["item"];
        (
            item["type"].as_str()?,
            item["id"].as_str()?.to_owned(),
            item["content"].clone(),
            item["phase"].as_str(),
        )
    } else {
        match p["type"].as_str()? {
            "user_message" => (
                "UserMessage",
                format!("saved-{offset}"),
                json!([{"text":p["message"]}]),
                None,
            ),
            "agent_message" => (
                "AgentMessage",
                format!("saved-{offset}"),
                json!([{"text":p["message"]}]),
                p["phase"].as_str(),
            ),
            _ => return None,
        }
    };
    if !["UserMessage", "AgentMessage"].contains(&kind) || phase == Some("analysis") {
        return None;
    }
    let raw = content
        .as_array()?
        .iter()
        .filter_map(|v| v["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let raw = if raw.is_empty() && kind == "UserMessage" {
        "[附件消息]".into()
    } else {
        raw
    };
    if raw.is_empty() {
        return None;
    }
    let text: String = raw.chars().take(24000).collect();
    let mut item = if kind == "UserMessage" {
        json!({"id":id,"type":"userMessage","content":[{"type":"text","text":text}]})
    } else {
        json!({"id":id,"type":"agentMessage","text":text})
    };
    item["timestamp"] = event["timestamp"].clone();
    item["historyCursor"] = json!(end.to_string());
    Some(item)
}

pub fn read(
    home: &Path,
    thread: &str,
    cursor: Option<&str>,
    users_only: bool,
) -> Result<Value, String> {
    catalog::verify_thread(home, thread)?;
    let db = catalog::connection(home)?;
    let (path, title, name, cwd, created, updated): (
        String,
        String,
        Option<String>,
        String,
        i64,
        i64,
    ) = db
        .query_row(
            "SELECT rollout_path,title,name,cwd,created_at,updated_at FROM threads WHERE id=?1",
            params![thread],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .map_err(|_| "找不到原会话历史")?;
    let path = Path::new(&path)
        .canonicalize()
        .map_err(|_| "原会话历史文件暂不可用")?;
    let home = home.canonicalize().map_err(|_| "原桌面目录不可用")?;
    if ![home.join("sessions"), home.join("archived_sessions")]
        .iter()
        .any(|p| path.starts_with(p))
    {
        return Err("原会话历史不在允许的 Codex 目录中".into());
    }
    let mut file = File::open(path).map_err(|_| "无法读取原会话历史")?;
    // Verify transcript identity without reading private context into the response.
    let mut header = vec![0; 1024 * 1024];
    let n = file.read(&mut header).map_err(|_| "历史读取失败")?;
    let first = header[..n]
        .split(|b| *b == b'\n')
        .next()
        .unwrap_or_default();
    let meta: Value = serde_json::from_slice(first).map_err(|_| "历史文件身份不兼容")?;
    if meta["type"] != "session_meta" || meta["payload"]["id"] != thread {
        return Err("历史文件与会话身份不匹配".into());
    }
    let size = file.metadata().map_err(|_| "历史读取失败")?.len();
    let end = cursor
        .map(str::parse::<u64>)
        .transpose()
        .map_err(|_| "历史分页位置无效")?
        .unwrap_or(size);
    if end > size {
        return Err("历史文件已变化，请重新打开会话".into());
    }
    let start = end.saturating_sub(WINDOW);
    file.seek(SeekFrom::Start(start))
        .map_err(|_| "历史读取失败")?;
    let mut bytes = vec![0; (end - start) as usize];
    file.read_exact(&mut bytes).map_err(|_| "历史读取失败")?;
    let boundary = if start > 0 {
        bytes
            .iter()
            .position(|b| *b == b'\n')
            .map(|n| n + 1)
            .unwrap_or(bytes.len())
    } else {
        0
    };
    let mut position = end;
    let mut next = start + boundary as u64;
    let mut items = vec![];
    let mut budget = 160000;
    let mut seen = std::collections::HashSet::new();
    for line in bytes[boundary..].split_inclusive(|b| *b == b'\n').rev() {
        let line_end = position;
        position -= line.len() as u64;
        // Incomplete final writes are retried on the next refresh, never cursor anchors.
        if !line.ends_with(b"\n") {
            continue;
        }
        let Ok(event) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        if let Some(item) = visible(&event, thread, position, line_end) {
            if users_only && item["type"] != "userMessage" {
                continue;
            }
            if !seen.insert(item["id"].as_str().unwrap_or_default().to_owned()) {
                continue;
            }
            let cost = item.to_string().len();
            if cost > budget && !items.is_empty() {
                next = line_end;
                break;
            }
            budget = budget.saturating_sub(cost);
            items.push(item);
            if items.len() >= 20 {
                next = position;
                break;
            }
        }
    }
    // A huge single tool record must not trap pagination on the same byte offset.
    if next >= end && end > 0 {
        next = start;
    }
    items.reverse();
    let turns:Vec<_>=items.into_iter().map(|item|json!({"id":format!("saved:{}",item["id"].as_str().unwrap_or_default()),"status":"completed","items":[item]})).collect();
    Ok(
        json!({"thread":{"id":thread,"preview":title,"name":name.unwrap_or(title.clone()),"cwd":cwd,
        "createdAt":created,"updatedAt":updated,"turns":turns,
        "desktop":{"source":"history","status":"unknown","canSend":false,"needsActivation":true,"observedAt":crate::now_ms()},
        "historyCursor":if next>0{Some(next.to_string())}else{None}},"nextCursor":if next>0{Some(next.to_string())}else{None}}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pages_only_completed_visible_items_and_never_internal_context() {
        let root = tempfile::tempdir().unwrap();
        let thread = "01a0ec64-bbfa-7861-b3fc-c3c171f7b1ed";
        let dir = root.path().join("sessions");
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("test.jsonl");
        let mut lines = vec![json!({"type":"session_meta","payload":{"id":thread}}).to_string()];
        lines.push(json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":"INTERNAL_INSTRUCTIONS"}]}}).to_string());
        for i in 0..25 {
            lines.push(json!({"type":"event_msg","payload":{"type":"item_completed","thread_id":thread,"item":{"id":format!("u{i}"),"type":"UserMessage","content":[{"type":"text","text":format!("message {i}")}]}}}).to_string());
            lines.push(json!({"type":"event_msg","payload":{"type":"item_completed","item":{"id":format!("a{i}"),"type":"AgentMessage","phase":"analysis","content":[{"text":"PRIVATE_REASONING"}]}}}).to_string());
        }
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let db = rusqlite::Connection::open(root.path().join("state_5.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE threads(id TEXT,archived INTEGER,source TEXT,rollout_path TEXT,title TEXT,name TEXT,cwd TEXT,created_at INTEGER,updated_at INTEGER)").unwrap();
        db.execute(
            "INSERT INTO threads VALUES(?1,0,'vscode',?2,'Test',NULL,'/repo',0,0)",
            params![thread, path.to_str().unwrap()],
        )
        .unwrap();
        let first = read(root.path(), thread, None, false).unwrap();
        assert_eq!(first["thread"]["turns"].as_array().unwrap().len(), 20);
        assert!(!first.to_string().contains("INTERNAL"));
        assert!(!first.to_string().contains("PRIVATE"));
        let second = read(root.path(), thread, first["nextCursor"].as_str(), true).unwrap();
        assert_eq!(second["thread"]["turns"].as_array().unwrap().len(), 5);
        assert!(second["nextCursor"].is_null());
        assert_eq!(first["thread"]["turns"][0]["items"][0]["id"], "u5");
        let cursor = first["thread"]["turns"][0]["items"][0]["historyCursor"].as_str();
        let around = read(root.path(), thread, cursor, false).unwrap();
        assert_eq!(
            around["thread"]["turns"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["items"][0]["id"],
            "u5"
        );
        assert!(read(root.path(), thread, Some("999999999999"), false).is_err());
    }
}
