//! Bounded metadata-only reminders. No transcript body leaves this endpoint.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Default)]
pub struct Notifications {
    files: HashMap<PathBuf, (u64, SystemTime, Vec<Value>)>,
}
const WINDOW: u64 = 1024 * 1024;

fn event(record: &Value, thread: &str, offset: u64) -> Option<Value> {
    let p = &record["payload"];
    let kind = match (record["type"].as_str()?, p["type"].as_str()?) {
        ("event_msg", "task_complete") => "completed",
        (
            "event_msg",
            "request_user_input" | "exec_approval_request" | "apply_patch_approval_request",
        ) => "attention",
        ("response_item", "function_call")
            if p["name"].as_str().is_some_and(|name| {
                name.rsplit("__")
                    .next()
                    .unwrap_or(name)
                    .rsplit('.')
                    .next()
                    .is_some_and(|n| matches!(n, "request_user_input" | "request_user_input_async"))
            }) =>
        {
            "attention"
        }
        _ => return None,
    };
    // Byte position is stable across repeated reads; include the source identity to avoid collisions.
    let id = format!("{:x}", Sha256::digest(format!("{thread}:{offset}:{kind}")));
    Some(json!({"id":id,"threadId":thread,"kind":kind,"timestamp":record["timestamp"]}))
}
fn scan(path: &Path, thread: &str) -> Result<Vec<Value>, String> {
    let mut f = File::open(path).map_err(|_| "提醒历史暂不可用")?;
    let size = f.metadata().map_err(|_| "提醒历史暂不可用")?.len();
    let mut header = vec![0; 1024 * 1024];
    let n = f.read(&mut header).map_err(|_| "提醒历史读取失败")?;
    let meta: Value = serde_json::from_slice(
        header[..n]
            .split(|b| *b == b'\n')
            .next()
            .unwrap_or_default(),
    )
    .map_err(|_| "提醒历史身份无效")?;
    if meta["type"] != "session_meta" || meta["payload"]["id"] != thread {
        return Err("提醒历史身份不匹配".into());
    }
    let start = size.saturating_sub(WINDOW);
    f.seek(SeekFrom::Start(start))
        .map_err(|_| "提醒历史读取失败")?;
    let mut bytes = vec![0; (size - start) as usize];
    f.read_exact(&mut bytes).map_err(|_| "提醒历史读取失败")?;
    let mut offset = start;
    let mut result = VecDeque::new();
    for (index, line) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
        let position = offset;
        offset += line.len() as u64;
        if (start > 0 && index == 0) || !line.ends_with(b"\n") {
            continue;
        }
        if let Ok(record) = serde_json::from_slice::<Value>(line) {
            if let Some(value) = event(&record, thread, position) {
                result.push_back(value);
                if result.len() > 20 {
                    result.pop_front();
                }
            }
        }
    }
    Ok(result.into_iter().collect())
}
impl Notifications {
    pub fn read(&mut self, home: &Path) -> Result<Value, String> {
        let home = home.canonicalize().map_err(|_| "Codex 目录不可用")?;
        let db = crate::catalog::connection(&home)?;
        let mut q=db.prepare("SELECT id,rollout_path FROM threads WHERE archived=0 AND source NOT LIKE '%subAgent%' ORDER BY updated_at DESC LIMIT 100").map_err(|_|"提醒索引不兼容")?;
        let rows = q
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|_| "提醒索引不可用")?;
        let mut events = Vec::new();
        let mut keep = Vec::new();
        for row in rows {
            let (id, raw) = row.map_err(|_| "提醒索引不可用")?;
            let Ok(path) = Path::new(&raw).canonicalize() else {
                continue;
            };
            if !path.starts_with(home.join("sessions")) {
                continue;
            }
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let Ok(modified) = meta.modified() else {
                continue;
            };
            keep.push(path.clone());
            if !self
                .files
                .get(&path)
                .is_some_and(|v| v.0 == meta.len() && v.1 == modified)
            {
                if let Ok(values) = scan(&path, &id) {
                    self.files
                        .insert(path.clone(), (meta.len(), modified, values));
                }
            }
            if let Some((_, _, values)) = self.files.get(&path) {
                events.extend(values.iter().cloned());
            }
        }
        self.files.retain(|path, _| keep.contains(path));
        events.sort_by(|a, b| a["timestamp"].as_str().cmp(&b["timestamp"].as_str()));
        if events.len() > 100 {
            events.drain(..events.len() - 100);
        }
        Ok(json!({"events":events,"observedAt":crate::now_ms()}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reminders_are_metadata_only_stable_and_ignore_internal_or_partial_output() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("thread.jsonl");
        let records = [
            json!({"type":"session_meta","payload":{"id":"thread"}}),
            json!({"timestamp":"2026-10-13T01:00:00Z","type":"event_msg","payload":{"type":"task_complete","last_agent_message":"SECRET"}}),
            json!({"timestamp":"2026-10-13T01:01:00Z","type":"response_item","payload":{"type":"function_call","name":"functions.request_user_input_async","arguments":"SECRET"}}),
            json!({"type":"response_item","payload":{"type":"reasoning","text":"SECRET"}}),
        ];
        let text = records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n{\"type\":\"event_msg\"";
        std::fs::write(&path, text).unwrap();
        let first = scan(&path, "thread").unwrap();
        assert_eq!(first.len(), 2);
        assert_eq!(first, scan(&path, "thread").unwrap());
        assert!(!json!(first).to_string().contains("SECRET"));
        assert!(scan(&path, "another-thread").is_err());
        assert_ne!(first[0]["id"], first[1]["id"]);
    }
}
