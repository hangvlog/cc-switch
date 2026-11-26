//! Ordinary, persistent Codex forks. A retry with the same receipt never forks twice.
use crate::{native, request_id, Bridge};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read},
    path::Path,
    sync::atomic::Ordering,
};

pub(crate) struct Forks(Connection);
impl Forks {
    pub(crate) fn open(storage: &Path) -> Result<Self, String> {
        let db = Connection::open(storage.join("remote-dispatches.sqlite"))
            .map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS thread_forks
          (request_id TEXT PRIMARY KEY,parent TEXT NOT NULL,anchor TEXT NOT NULL,child TEXT,status TEXT NOT NULL)").map_err(|e|e.to_string())?;
        Ok(Self(db))
    }
}

impl Bridge {
    pub(crate) fn fork_thread(&self, params: &Value) -> Result<Value, String> {
        let object = params.as_object().ok_or("分支参数无效")?;
        if object
            .keys()
            .any(|k| !["threadId", "requestId", "lastTurnId", "beforeTurnId"].contains(&k.as_str()))
        {
            return Err("分支参数无效".into());
        }
        let parent = self.thread(params)?;
        let id = request_id(params)?;
        if !params["lastTurnId"].is_null() && !params["beforeTurnId"].is_null() {
            return Err("请选择一个分支起点".into());
        }
        let kind = if params["beforeTurnId"].is_null() {
            "lastTurnId"
        } else {
            "beforeTurnId"
        };
        let anchor = match params.get(kind) {
            None | Some(Value::Null) => "",
            Some(Value::String(s)) if uuid::Uuid::parse_str(s).is_ok() => s.as_str(),
            _ => return Err("请选择原对话中的轮次".into()),
        };
        let identity = format!("{kind}:{anchor}");
        let db = &self.forks.0;
        let existing: Option<(String, String, Option<String>, String)> = db
            .query_row(
                "SELECT parent,anchor,child,status FROM thread_forks WHERE request_id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some((p, a, child, status)) = existing {
            if p != parent || a != identity {
                return Err("此分支请求已用于其他轮次".into());
            }
            return Ok(json!({"status":status,"threadId":child,"forkedFromId":parent}));
        }
        if !native::fork_ready(&self.home) {
            return Ok(json!({"status":"unavailable"}));
        }
        if !self.enabled.load(Ordering::SeqCst) {
            return Err("远程连接已关闭".into());
        }
        db.execute(
            "INSERT INTO thread_forks VALUES(?1,?2,?3,NULL,'unknown')",
            params![id, parent, identity],
        )
        .map_err(|e| e.to_string())?;
        let mut args = json!({"threadId":parent,"excludeTurns":true,"deferGoalContinuation":true});
        if !anchor.is_empty() {
            args[kind] = json!(anchor);
        }
        match native::request_outcome(&self.home, "thread/fork", args) {
            Ok(reply)
                if reply["thread"]["forkedFromId"] == parent
                    && reply["thread"]["id"]
                        .as_str()
                        .is_some_and(|c| c != parent && uuid::Uuid::parse_str(c).is_ok()) =>
            {
                let child = reply["thread"]["id"].as_str().unwrap();
                db.execute(
                    "UPDATE thread_forks SET child=?2,status='accepted' WHERE request_id=?1",
                    params![id, child],
                )
                .map_err(|e| e.to_string())?;
                Ok(json!({"status":"accepted","threadId":child,"forkedFromId":parent}))
            }
            Err(error) if error.definite => {
                db.execute(
                    "UPDATE thread_forks SET status='rejected' WHERE request_id=?1",
                    [id],
                )
                .map_err(|e| e.to_string())?;
                Ok(json!({"status":"rejected","reason":error.message}))
            }
            _ => Ok(json!({"status":"unknown"})),
        }
    }
}

// Codex records ordinary forks in the first rollout metadata record. Attached
// parents and spawned agents are separate relationships.
pub(crate) fn parent(home: &Path, id: &str, path: &str) -> Option<String> {
    let path = Path::new(path).canonicalize().ok()?;
    if !path.starts_with(home.canonicalize().ok()?.join("sessions")) {
        return None;
    }
    let mut first = Vec::new();
    BufReader::new(std::fs::File::open(path).ok()?)
        .take(256 * 1024)
        .read_until(b'\n', &mut first)
        .ok()?;
    let data: Value = serde_json::from_slice(&first).ok()?;
    if data["type"] != "session_meta" || data["payload"]["id"] != id {
        return None;
    }
    let parent = data["payload"]["forked_from_id"].as_str()?;
    uuid::Uuid::parse_str(parent)
        .ok()
        .map(|_| parent.to_owned())
}
