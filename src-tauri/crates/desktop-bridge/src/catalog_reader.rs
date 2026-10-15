//! Stateless sidebar reads must not wait behind owner discovery or task commands.
use serde_json::{json, Value};
use std::{path::{Path, PathBuf}, sync::{Arc, atomic::{AtomicBool, Ordering}}};

#[derive(Clone)]
pub struct CatalogReader {
    home: PathBuf,
    enabled: Arc<AtomicBool>,
}

impl CatalogReader {
    pub(crate) fn new(home: &Path, enabled: Arc<AtomicBool>) -> Self {
        Self { home: home.into(), enabled }
    }

    pub fn accepts(request: &Value) -> bool {
        matches!(request["method"].as_str(), Some("desktop/thread/list" | "desktop/thread/statuses"))
    }

    pub fn handle(&self, request: Value) -> Option<Value> {
        if !Self::accepts(&request) { return None; }
        let id = request.get("id")?.clone();
        if !(id.is_string() || id.is_i64() || id.is_u64()) { return None; }
        let result = if !self.enabled.load(Ordering::SeqCst) {
            Err("远程连接已关闭".into())
        } else {
            match request["method"].as_str() {
                Some("desktop/thread/list") => list(&self.home, &request["params"]),
                _ => crate::thread_status::read(&self.home, &request["params"]),
            }
        };
        Some(match result {
            Ok(value) => json!({"id":id,"result":value}),
            Err(message) => json!({"id":id,"error":{"code":-32001,"message":message,"data":{"notSent":false}}}),
        })
    }
}

pub(crate) fn list(home: &Path, params: &Value) -> Result<Value, String> {
    let offset = params["cursor"].as_str().unwrap_or("0").parse::<usize>()
        .map_err(|_| "分页游标无效")?;
    if offset > 100_000 { return Err("分页游标超出范围".into()); }
    crate::catalog::page(home, offset, params["limit"].as_u64().unwrap_or(50).min(50) as usize,
        params["lightweight"] == true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_remains_available_while_command_bridge_is_locked() {
        let root = tempfile::tempdir().unwrap();
        let db = rusqlite::Connection::open(root.path().join("state_5.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE threads(id TEXT,title TEXT,cwd TEXT,created_at INTEGER,updated_at INTEGER,archived INTEGER,source TEXT);
            INSERT INTO threads VALUES('one','hello','/repo',0,1,0,'vscode');").unwrap();
        let bridge = std::sync::Mutex::new(crate::Bridge::open(root.path(), &root.path().join("remote")).unwrap());
        let held = bridge.lock().unwrap();
        let reader = held.catalog_reader();
        let (tx, rx) = std::sync::mpsc::channel();
        let task = std::thread::spawn(move || tx.send(reader.handle(json!({"id":"list-1","method":"desktop/thread/list","params":{"limit":20,"lightweight":true}}))).unwrap());
        let response = rx.recv_timeout(std::time::Duration::from_secs(2)).expect("list waited behind a command").unwrap();
        assert_eq!(response["id"], "list-1");
        assert_eq!(response["result"]["data"][0]["preview"], "hello");
        assert!(response["result"]["data"][0]["latestReply"].is_null());
        task.join().unwrap();
        let reader = held.catalog_reader();
        assert!(reader.handle(json!({"id":1,"method":"desktop/turn/start"})).is_none());
        assert!(reader.handle(json!({"id":{},"method":"desktop/thread/list"})).is_none());
        assert_eq!(reader.handle(json!({"id":2,"method":"desktop/thread/list","params":{"cursor":"bad"}})).unwrap()["error"]["message"],"分页游标无效");
        held.enable_flag().store(false, Ordering::SeqCst);
        for method in ["desktop/thread/list", "desktop/thread/statuses"] {
            assert_eq!(reader.handle(json!({"id":3,"method":method})).unwrap()["error"]["message"], "远程连接已关闭");
        }
    }
}
