//! Durable ClawKit outbox. Never replaces Codex's own queued-followups list.
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

pub struct Queue(Connection);
impl Queue {
    pub fn open(storage: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(storage).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(storage, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        let path = storage.join("remote-queue.sqlite");
        let db = Connection::open(&path).map_err(|e| e.to_string())?;
        db.busy_timeout(std::time::Duration::from_secs(2))
            .map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS queue (id TEXT PRIMARY KEY, thread TEXT NOT NULL, text TEXT NOT NULL,
            hash TEXT NOT NULL,status TEXT NOT NULL,created INTEGER NOT NULL,reason TEXT);").map_err(|e|e.to_string())?;
        let has_images: bool = db
            .prepare("PRAGMA table_info(queue)")
            .map_err(|e| e.to_string())?
            .query_map([], |r| r.get::<_, String>(1))
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .any(|name| name == "images");
        if !has_images {
            db.execute(
                "ALTER TABLE queue ADD COLUMN images TEXT NOT NULL DEFAULT '[]'",
                [],
            )
            .map_err(|e| e.to_string())?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
        }
        Ok(Self(db))
    }
    pub fn existing(&self, id: &str, thread: &str, text: &str) -> Result<Option<Value>, String> {
        let row: Option<(String, String)> = self
            .0
            .query_row("SELECT thread,hash FROM queue WHERE id=?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some((old_thread, hash)) = row {
            if old_thread != thread || hash != format!("{:x}", Sha256::digest(text.as_bytes())) {
                return Err("请求编号已用于不同内容".into());
            }
            return self.receipt(id, thread);
        }
        Ok(None)
    }
    #[cfg(test)]
    pub fn add(&self, id: &str, thread: &str, text: &str) -> Result<Value, String> {
        self.add_images(id, thread, text, &[])
    }
    pub fn add_images(
        &self,
        id: &str,
        thread: &str,
        text: &str,
        images: &[String],
    ) -> Result<Value, String> {
        let identity = crate::attachments::Message {
            text: text.into(),
            images: images.into(),
        }
        .identity();
        if let Some(old) = self.existing(id, thread, &identity)? {
            return Ok(old);
        }
        let count: i64 = self
            .0
            .query_row(
                "SELECT COUNT(*) FROM queue WHERE status IN ('queued','unknown','rejected')",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if count >= 100 {
            return Err("队列已满，请先处理待发送消息".into());
        }
        self.0
            .execute(
                "INSERT INTO queue (id,thread,text,hash,status,created,reason,images) VALUES (?1,?2,?3,?4,'queued',?5,NULL,?6)",
                params![
                    id,
                    thread,
                    text,
                    format!("{:x}", Sha256::digest(identity.as_bytes())),
                    crate::now_ms(),
                    json!(images).to_string()
                ],
            )
            .map_err(|e| e.to_string())?;
        self.receipt(id, thread)?.ok_or("队列回执不可用".into())
    }
    pub fn receipt(&self, id: &str, thread: &str) -> Result<Option<Value>, String> {
        self.0.query_row("SELECT status,reason FROM queue WHERE id=?1 AND thread=?2",params![id,thread],|r|Ok(json!({"requestId":id,"threadId":thread,"status":r.get::<_,String>(0)?,"reason":r.get::<_,Option<String>>(1)?}))).optional().map_err(|e|e.to_string())
    }
    pub fn list(&self, thread: &str) -> Result<Vec<Value>, String> {
        let mut q=self.0.prepare("SELECT id,text,status,reason,images FROM queue WHERE thread=?1 AND status IN ('queued','unknown','rejected') ORDER BY created,rowid").map_err(|e|e.to_string())?;
        let rows=q.query_map([thread],|r|Ok(json!({"id":r.get::<_,String>(0)?,"text":r.get::<_,String>(1)?,"imageCount":serde_json::from_str::<Vec<String>>(&r.get::<_,String>(4)?).unwrap_or_default().len(),"status":r.get::<_,String>(2)?,"reason":r.get::<_,Option<String>>(3)?}))).map_err(|e|e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }
    pub fn heads(&self) -> Result<Vec<(String, String, String, Vec<String>)>, String> {
        let mut q=self.0.prepare("SELECT id,thread,text,images FROM queue q WHERE status IN ('queued','unknown') AND NOT EXISTS (SELECT 1 FROM queue p WHERE p.thread=q.thread AND p.status IN ('queued','unknown','rejected') AND p.rowid<q.rowid) ORDER BY rowid LIMIT 10").map_err(|e|e.to_string())?;
        let rows = q
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default(),
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }
    pub fn mark(&self, id: &str, status: &str, reason: Option<&str>) -> Result<(), String> {
        self.0.execute("UPDATE queue SET status=?2,reason=?3,text=CASE WHEN ?2 IN ('accepted','cancelled') THEN '' ELSE text END,images=CASE WHEN ?2 IN ('accepted','cancelled') THEN '[]' ELSE images END WHERE id=?1",params![id,status,reason]).map_err(|e|e.to_string())?;
        Ok(())
    }
    pub fn cancel(&self, id: &str, thread: &str) -> Result<Value, String> {
        let old = self.receipt(id, thread)?.ok_or("未找到排队消息")?;
        if old["status"] == "unknown" {
            return Err("发送结果待核对，不能撤销已发出的请求".into());
        }
        if old["status"] == "queued" || old["status"] == "rejected" {
            self.mark(id, "cancelled", None)?;
        }
        self.receipt(id, thread)?.ok_or("未找到排队消息".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persists_orders_deduplicates_and_cancels() {
        let d = tempfile::tempdir().unwrap();
        let q = Queue::open(d.path()).unwrap();
        q.add("1", "t", "first").unwrap();
        let images = vec!["a".repeat(64)];
        q.add_images("2", "t", "second", &images).unwrap();
        q.add("1", "t", "first").unwrap();
        assert!(q.add("1", "t", "changed").is_err());
        assert_eq!(q.list("t").unwrap().len(), 2);
        drop(q);
        let q = Queue::open(d.path()).unwrap();
        assert_eq!(q.heads().unwrap()[0].0, "1");
        q.mark("1", "unknown", None).unwrap();
        assert!(q.cancel("1", "t").is_err());
        q.mark("1", "accepted", None).unwrap();
        assert_eq!(q.heads().unwrap()[0].0, "2");
        assert_eq!(q.heads().unwrap()[0].3, images);
        assert_eq!(q.list("t").unwrap()[0]["imageCount"], 1);
        q.cancel("2", "t").unwrap();
        assert!(q.heads().unwrap().is_empty());
    }
}
