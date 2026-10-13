//! Bounded durable metadata, scoped to the current local engine authentication file.
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::VecDeque, io::Read, path::Path};

pub(super) struct Reminders { db: Connection, scope: String }
impl Reminders {
    pub fn open(home: &Path) -> Result<Self, String> {
        let mut auth = Vec::new();
        if let Ok(file) = std::fs::File::open(home.join("auth.json")) {
            file.take(128 * 1024 + 1).read_to_end(&mut auth).map_err(|e| e.to_string())?;
            if auth.len() > 128 * 1024 { return Err("身份文件过大".into()); }
        }
        // Hash only; tokens and transcript text are never stored. A token refresh may
        // conservatively start a new reminder bucket; an account change never reuses one.
        let scope = format!("{:x}", Sha256::digest(&auth));
        let directory = home.join("clawkit-runtime");
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let path = directory.join("reminders.sqlite");
        let db = Connection::open(&path).map_err(|e| e.to_string())?;
        db.busy_timeout(std::time::Duration::from_millis(200)).map_err(|e| e.to_string())?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS reminders(id TEXT PRIMARY KEY, scope TEXT NOT NULL, thread TEXT NOT NULL, kind TEXT NOT NULL, timestamp INTEGER NOT NULL);").map_err(|e| e.to_string())?;
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
        }
        Ok(Self { db, scope })
    }
    pub fn read(&self) -> Result<VecDeque<Value>, String> {
        let mut query = self.db.prepare("SELECT id,thread,kind,timestamp FROM (SELECT * FROM reminders WHERE scope=?1 ORDER BY timestamp DESC,rowid DESC LIMIT 100) ORDER BY timestamp ASC").map_err(|e| e.to_string())?;
        let rows = query.query_map([&self.scope], |r| Ok(json!({"id":r.get::<_,String>(0)?,"threadId":r.get::<_,String>(1)?,"kind":r.get::<_,String>(2)?,"timestampMs":r.get::<_,u64>(3)?}))).map_err(|e|e.to_string())?;
        rows.collect::<Result<_,_>>().map_err(|e|e.to_string())
    }
    pub fn save(&mut self, event: &Value) -> Result<(), String> {
        let transaction = self.db.transaction().map_err(|e|e.to_string())?;
        transaction.execute("INSERT OR IGNORE INTO reminders VALUES(?1,?2,?3,?4,?5)", params![event["id"].as_str(),self.scope,event["threadId"].as_str(),event["kind"].as_str(),event["timestampMs"].as_u64()]).map_err(|e|e.to_string())?;
        // Metadata cache only; cap total records across authentication changes too.
        transaction.execute("DELETE FROM reminders WHERE rowid NOT IN (SELECT rowid FROM reminders ORDER BY timestamp DESC,rowid DESC LIMIT 100)",[]).map_err(|e|e.to_string())?;
        transaction.commit().map_err(|e|e.to_string())
    }
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn survives_restart_is_bounded_and_does_not_cross_authentication() {
        let root=tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("auth.json"),"synthetic-account-a").unwrap();
        let mut store=Reminders::open(root.path()).unwrap();
        for i in 0..102 {store.save(&json!({"id":i.to_string(),"threadId":"thread","kind":"completed","timestampMs":i})).unwrap();}
        drop(store);
        let rows=Reminders::open(root.path()).unwrap().read().unwrap();
        assert_eq!(rows.len(),100);assert_eq!(rows.front().unwrap()["id"],"2");
        assert_eq!(rows.back().unwrap()["id"],"101");
        std::fs::write(root.path().join("auth.json"),"synthetic-account-b").unwrap();
        assert!(Reminders::open(root.path()).unwrap().read().unwrap().is_empty());
    }
}
