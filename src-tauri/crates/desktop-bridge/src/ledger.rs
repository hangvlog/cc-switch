use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

pub struct Ledger(Connection);
impl Ledger {
    pub fn has_request(&self, id: &str) -> Result<bool, String> {
        self.0
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM dispatches WHERE request_id=?1)",
                [id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())
    }
    pub fn open(directory: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        let path = directory.join("remote-dispatches.sqlite");
        let db = Connection::open(&path).map_err(|e| e.to_string())?;
        db.busy_timeout(std::time::Duration::from_secs(2))
            .map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS dispatches (request_id TEXT PRIMARY KEY,thread_id TEXT NOT NULL,text_hash TEXT NOT NULL,
            status TEXT NOT NULL,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL);
            CREATE UNIQUE INDEX IF NOT EXISTS unresolved_thread ON dispatches(thread_id) WHERE status IN ('pending','unknown');")
            .map_err(|e|e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
        }
        Ok(Self(db))
    }

    pub fn existing(&self, id: &str, thread: &str, text: &str) -> Result<Option<Value>, String> {
        let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
        let old: Option<(String, String)> = self
            .0
            .query_row(
                "SELECT thread_id,text_hash FROM dispatches WHERE request_id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some((original_thread, original_hash)) = old {
            if original_thread != thread || original_hash != hash {
                return Err("请求编号已用于不同内容，不能重用".into());
            }
            return self.read(id, thread).map(Some);
        }
        Ok(None)
    }

    pub fn begin(&self, id: &str, thread: &str, text: &str) -> Result<Option<Value>, String> {
        if let Some(receipt) = self.existing(id, thread, text)? {
            return Ok(Some(receipt));
        }
        let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
        self.0
            .execute(
                "INSERT INTO dispatches VALUES (?1,?2,?3,'pending',?4,?4)",
                params![id, thread, hash, crate::now_ms()],
            )
            .map_err(|_| "此任务有尚未确认的发送，请先核对回执")?;
        Ok(None)
    }

    pub fn mark(&self, id: &str, status: &str) -> Result<(), String> {
        self.0
            .execute(
                "UPDATE dispatches SET status=?1,updated_at=?2 WHERE request_id=?3",
                params![status, crate::now_ms(), id],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn unresolved(&self, thread: &str) -> Result<Vec<String>, String> {
        let mut query=self.0.prepare("SELECT request_id FROM dispatches WHERE thread_id=?1 AND status IN ('pending','unknown')").map_err(|e|e.to_string())?;
        let rows = query
            .query_map([thread], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    pub fn read(&self, id: &str, thread: &str) -> Result<Value, String> {
        self.0.query_row("SELECT status,updated_at FROM dispatches WHERE request_id=?1 AND thread_id=?2",params![id,thread],|row| {
            let status:String=row.get(0)?;
            Ok(json!({"requestId":id,"threadId":thread,"status":if status=="pending"{"unknown"}else{&status},"updatedAt":row.get::<_,i64>(1)?}))
        }).map_err(|_| "未找到发送回执，请使用原请求编号核对".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn durable_deduplication_and_unknown_blocks_resend() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        assert!(ledger
            .begin("one", "thread", "private text")
            .unwrap()
            .is_none());
        assert_eq!(
            ledger
                .begin("one", "thread", "private text")
                .unwrap()
                .unwrap()["status"],
            "unknown"
        );
        assert!(ledger.begin("one", "thread", "different").is_err());
        assert!(ledger.begin("two", "thread", "private text").is_err());
        drop(ledger);
        let ledger = Ledger::open(dir.path()).unwrap();
        assert_eq!(ledger.read("one", "thread").unwrap()["status"], "unknown");
        ledger.mark("one", "accepted").unwrap();
        assert!(ledger.begin("two", "thread", "next").unwrap().is_none());
        let db = std::fs::read(dir.path().join("remote-dispatches.sqlite")).unwrap();
        assert!(!String::from_utf8_lossy(&db).contains("private text"));
    }
}
