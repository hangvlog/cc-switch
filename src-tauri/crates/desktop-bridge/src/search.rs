//! On-demand, incremental index of visible user messages only. Never indexes
//! system context, reasoning, tool output or assistant responses.
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

pub struct Search {
    database: PathBuf,
}
const BUDGET: u64 = 128 * 1024 * 1024;
const LINE_LIMIT: usize = 4 * 1024 * 1024;

fn user_text(event: &Value, thread: &str) -> Option<String> {
    if event["type"] != "event_msg" {
        return None;
    }
    let p = &event["payload"];
    match p["type"].as_str()? {
        "user_message" => p["message"].as_str().map(str::to_owned),
        "item_completed"
            if p["item"]["type"] == "UserMessage"
                && !p["thread_id"].as_str().is_some_and(|id| id != thread) =>
        {
            Some(
                p["item"]["content"]
                    .as_array()?
                    .iter()
                    .filter_map(|v| v["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        }
        _ => None,
    }
}

// Read a whole line without allocating arbitrarily large tool records.
fn line(reader: &mut impl BufRead, out: &mut Vec<u8>) -> Result<(usize, bool), String> {
    out.clear();
    let mut length = 0;
    loop {
        let bytes = reader.fill_buf().map_err(|_| "无法读取会话搜索历史")?;
        if bytes.is_empty() {
            return Ok((length, false));
        }
        let end = bytes.iter().position(|b| *b == b'\n').map(|i| i + 1);
        let count = end.unwrap_or(bytes.len());
        if length + count <= LINE_LIMIT {
            out.extend_from_slice(&bytes[..count]);
        } else {
            out.clear();
        }
        length += count;
        reader.consume(count);
        if end.is_some() {
            return Ok((length, true));
        }
    }
}

fn snippet(text: &str, term: &str) -> String {
    let lower = text.to_lowercase();
    let offset = lower
        .find(term)
        .map(|i| lower[..i].chars().count())
        .unwrap_or(0);
    let start = offset.saturating_sub(45);
    let chars: Vec<_> = text.chars().collect();
    let end = (start + 220).min(chars.len());
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        chars[start.min(chars.len())..end]
            .iter()
            .collect::<String>(),
        if end < chars.len() { "…" } else { "" }
    )
}

impl Search {
    pub fn open(storage: &Path) -> Result<Self, String> {
        let value = Self {
            database: storage.join("user-message-search.sqlite"),
        };
        let db = value.connect()?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS files(thread TEXT PRIMARY KEY,path TEXT,offset INTEGER,modified INTEGER);
            CREATE TABLE IF NOT EXISTS messages(thread TEXT,cursor INTEGER,text TEXT,normalized TEXT,PRIMARY KEY(thread,cursor));
            CREATE INDEX IF NOT EXISTS messages_thread ON messages(thread);").map_err(|_| "无法初始化会话搜索")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&value.database, std::fs::Permissions::from_mode(0o600))
                .map_err(|_| "无法保护会话搜索缓存")?;
        }
        Ok(value)
    }
    fn connect(&self) -> Result<Connection, String> {
        Connection::open(&self.database).map_err(|_| "无法打开会话搜索缓存".into())
    }
    pub fn query(&self, home: &Path, query: &str) -> Result<Value, String> {
        let query = query.trim().to_lowercase();
        if query.is_empty() || query.chars().count() > 200 {
            return Err("请输入 1 到 200 个字符进行搜索".into());
        }
        let terms: Vec<_> = query.split_whitespace().collect();
        let source = crate::catalog::connection(home)?;
        let mut stmt = source.prepare("SELECT id,rollout_path FROM threads WHERE archived=0 AND source NOT LIKE '%subAgent%' ORDER BY updated_at DESC,id").map_err(|_| "会话索引不兼容")?;
        let files = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let home = home.canonicalize().map_err(|_| "Codex 目录不可用")?;
        let mut db = self.connect()?;
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let mut budget = BUDGET;
        let mut complete = 0;
        let mut unavailable = 0;
        for (thread, path) in &files {
            match Self::index(&tx, &home, thread, path, &mut budget) {
                Ok(true) => complete += 1,
                Ok(false) => {}
                Err(_) => {
                    unavailable += 1;
                    tx.execute("DELETE FROM messages WHERE thread=?1", [thread])
                        .map_err(|e| e.to_string())?;
                    tx.execute("DELETE FROM files WHERE thread=?1", [thread])
                        .map_err(|e| e.to_string())?;
                }
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        let eligible: std::collections::HashSet<_> =
            files.iter().map(|(id, _)| id.as_str()).collect();
        let predicates = (1..=terms.len())
            .map(|i| format!("instr(normalized,?{i})>0"))
            .collect::<Vec<_>>()
            .join(" AND ");
        let mut stmt = db
            .prepare(&format!(
                "SELECT thread,cursor,text FROM messages WHERE {predicates} ORDER BY cursor DESC"
            ))
            .map_err(|e| e.to_string())?;
        let matches = stmt
            .query_map(rusqlite::params_from_iter(&terms), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, u64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut by_thread = std::collections::HashMap::new();
        for row in matches {
            let (thread, cursor, text) = row.map_err(|e| e.to_string())?;
            if eligible.contains(thread.as_str()) {
                by_thread.entry(thread).or_insert_with(|| json!({"snippet":snippet(&text,terms[0]),"historyCursor":cursor.to_string()}));
            }
        }
        let mut data = vec![];
        let mut offset = 0;
        loop {
            let catalog = crate::catalog::list(&home, offset)?;
            for mut thread in catalog["data"].as_array().into_iter().flatten().cloned() {
                if let Some(hit) = thread["id"].as_str().and_then(|id| by_thread.remove(id)) {
                    thread["searchMatch"] = hit;
                    data.push(thread);
                }
            }
            if catalog["nextCursor"].is_null() || by_thread.is_empty() {
                break;
            }
            offset += 50;
        }
        let total = data.len();
        data.truncate(100);
        Ok(
            json!({"data":data,"total":total,"indexing":complete+unavailable<files.len(),"indexedThreads":complete,"totalThreads":files.len(),"unavailableThreads":unavailable}),
        )
    }
    fn index(
        db: &Connection,
        home: &Path,
        thread: &str,
        raw: &str,
        budget: &mut u64,
    ) -> Result<bool, String> {
        let path = Path::new(raw).canonicalize().map_err(|_| "历史暂不可用")?;
        if ![home.join("sessions"), home.join("archived_sessions")]
            .iter()
            .any(|root| path.starts_with(root))
        {
            return Err("历史路径不匹配".into());
        }
        let file = File::open(&path).map_err(|_| "历史暂不可用")?;
        let meta = file.metadata().map_err(|_| "历史元数据不可用")?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let previous = db
            .query_row(
                "SELECT path,offset,modified FROM files WHERE thread=?1",
                [thread],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, u64>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                },
            )
            .ok();
        let mut offset = previous.as_ref().map(|p| p.1).unwrap_or(0);
        if previous.as_ref().is_some_and(|p| {
            p.0 != raw || offset > meta.len() || (offset == meta.len() && p.2 != modified)
        }) {
            db.execute("DELETE FROM messages WHERE thread=?1", [thread])
                .map_err(|e| e.to_string())?;
            offset = 0;
        }
        if offset == meta.len() {
            return Ok(true);
        }
        if *budget == 0 {
            return Ok(false);
        }
        let mut reader = BufReader::new(file);
        let mut header = String::new();
        reader
            .by_ref()
            .take(1024 * 1024)
            .read_line(&mut header)
            .map_err(|_| "历史身份不可用")?;
        let header: Value = serde_json::from_str(&header).map_err(|_| "历史身份无效")?;
        if header["type"] != "session_meta" || header["payload"]["id"] != thread {
            return Err("历史身份不匹配".into());
        }
        reader
            .seek(SeekFrom::Start(offset))
            .map_err(|_| "历史读取失败")?;
        let mut bytes = vec![];
        let mut partial = false;
        while *budget > 0 && offset < meta.len() {
            let (len, ended) = line(&mut reader, &mut bytes)?;
            *budget = budget.saturating_sub(len as u64);
            if !ended {
                partial = true;
                break;
            }
            offset += len as u64;
            if len > LINE_LIMIT {
                continue;
            }
            // Cheap prefilter before JSON parsing; JSON identity/type checks below
            // still reject embedded user-looking strings inside tool output.
            if !bytes.windows(12).any(|v| v == b"user_message")
                && !bytes.windows(11).any(|v| v == b"UserMessage")
            {
                continue;
            }
            let Ok(event) = serde_json::from_slice(&bytes) else {
                continue;
            };
            if let Some(text) = user_text(&event, thread).filter(|s| !s.is_empty()) {
                db.execute(
                    "INSERT OR REPLACE INTO messages VALUES(?1,?2,?3,?4)",
                    params![thread, offset, text, text.to_lowercase()],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        db.execute(
            "INSERT OR REPLACE INTO files VALUES(?1,?2,?3,?4)",
            params![thread, raw, offset, modified],
        )
        .map_err(|e| e.to_string())?;
        Ok(partial || offset >= meta.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_actual_user_queries_are_searchable() {
        assert_eq!(user_text(&json!({"type":"event_msg","payload":{"type":"user_message","message":"旧消息片段"}}),"t").as_deref(),Some("旧消息片段"));
        for e in [
            json!({"type":"response_item","payload":{"role":"user","content":[{"text":"system context"}]}}),
            json!({"type":"event_msg","payload":{"type":"agent_message","message":"assistant"}}),
            json!({"type":"event_msg","payload":{"type":"item_completed","thread_id":"other","item":{"type":"UserMessage","content":[{"text":"other user"}]}}}),
        ] {
            assert!(user_text(&e, "t").is_none());
        }
    }
    #[test]
    fn index_finds_old_queries_appends_and_ignores_partial_writes() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("sessions")).unwrap();
        let path = root.path().join("sessions/test.jsonl");
        let header = json!({"type":"session_meta","payload":{"id":"t"}}).to_string() + "\n";
        let message=json!({"type":"event_msg","payload":{"type":"user_message","message":"微信里复制的内容"}}).to_string()+"\n";
        std::fs::write(&path, header + &message).unwrap();
        let search = Search::open(root.path()).unwrap();
        let db = search.connect().unwrap();
        let mut budget = BUDGET;
        assert!(Search::index(
            &db,
            &root.path().canonicalize().unwrap(),
            "t",
            path.to_str().unwrap(),
            &mut budget
        )
        .unwrap());
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM messages WHERE instr(normalized,'复制')>0",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(message.trim_end().as_bytes()).unwrap();
        assert!(Search::index(
            &db,
            &root.path().canonicalize().unwrap(),
            "t",
            path.to_str().unwrap(),
            &mut budget
        )
        .unwrap());
        f.write_all(b"\n").unwrap();
        assert!(Search::index(
            &db,
            &root.path().canonicalize().unwrap(),
            "t",
            path.to_str().unwrap(),
            &mut budget
        )
        .unwrap());
        assert_eq!(
            db.query_row("SELECT count(*) FROM messages", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert!(snippet(&"一".repeat(300), "一").chars().count() <= 221);
    }
}
