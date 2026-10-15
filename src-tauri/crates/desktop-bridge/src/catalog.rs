use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::path::Path;

pub(crate) fn connection(home: &Path) -> Result<Connection, String> {
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
    page(home, offset, 50, false)
}

pub fn page(home: &Path, offset: usize, limit: usize, lightweight: bool) -> Result<Value, String> {
    let limit = limit.clamp(1, 50);
    let connection = connection(home)?;
    let mut columns = connection
        .prepare("PRAGMA table_info(threads)")
        .map_err(|e| e.to_string())?;
    let columns = columns
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let modern = [
        "name",
        "project_id",
        "is_pinned",
        "section_position",
        "thread_section_id",
    ]
    .iter()
    .all(|column| columns.iter().any(|name| name == column));
    let extra = if modern {
        "t.name,t.project_id,(t.is_pinned=1 OR EXISTS(SELECT 1 FROM thread_sections s WHERE s.id=t.thread_section_id AND s.name='Pinned')),t.section_position"
    } else {
        "NULL,NULL,0,NULL"
    };
    // Recency advances for user activity; updated_at also changes during background
    // indexing and migration, which must not move old chats to the top of Recents.
    let activity = if columns.iter().any(|name| name == "recency_at_ms") {
        "COALESCE(t.recency_at_ms / 1000.0,t.recency_at,t.updated_at)"
    } else if columns.iter().any(|name| name == "recency_at") {
        "COALESCE(t.recency_at,t.updated_at)"
    } else {
        "t.updated_at"
    };
    let transcript = if columns.iter().any(|c|c=="rollout_path") { "t.rollout_path" } else { "NULL" };
    let thread_source = if columns.iter().any(|c|c=="thread_source") { "t.thread_source" } else { "NULL" };
    let sql = format!("SELECT t.id,t.title,t.cwd,t.created_at,{activity},{extra},{transcript},{thread_source} FROM threads t WHERE t.archived=0 AND t.source NOT LIKE '%subAgent%' ORDER BY {activity} DESC,t.id LIMIT ?2 OFFSET ?1");
    let mut query = connection
        .prepare(&sql)
        .map_err(|_| "Codex 任务索引结构已变化")?;
    let rows = query.query_map([offset as i64, (limit + 1) as i64],|row| Ok(json!({"id":row.get::<_,String>(0)?,
        "preview":row.get::<_,String>(1)?,"cwd":row.get::<_,String>(2)?,"createdAt":row.get::<_,i64>(3)?,
        "updatedAt":row.get::<_,f64>(4)?,"name":row.get::<_,Option<String>>(5)?,
        "projectId":row.get::<_,Option<String>>(6)?,"isPinned":row.get::<_,bool>(7)?,
        "pinnedPosition":row.get::<_,Option<i64>>(8)?,"threadSource":row.get::<_,Option<String>>(10)?,"_rollout":row.get::<_,Option<String>>(9)?,"turns":[],"desktop":{"source":"catalog","status":"unknown","canSend":false}})))
        .map_err(|e| e.to_string())?;
    let mut data = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let next = if data.len() > limit {
        data.truncate(limit);
        Some((offset + limit).to_string())
    } else {
        None
    };
    for thread in &mut data {
        if !lightweight { if let (Some(id),Some(path))=(thread["id"].as_str(),thread["_rollout"].as_str()) {
            if let Some(activity)=crate::activity::read(home,id,path) {
                thread["latestReply"]=activity["summary"].clone();
            }
        }
        }
        thread.as_object_mut().unwrap().remove("_rollout");
        if lightweight {
            if let Some(preview)=thread["preview"].as_str() {thread["preview"]=json!(preview.chars().take(160).collect::<String>());}
        }
    }
    crate::unread::annotate(home,&mut data);
    crate::activity_automations::annotate(home, &mut data);
    let sidebar = if modern {
        let projects = crate::sidebar::projects(&connection)?;
        crate::sidebar::assign_projects(home, &mut data, &projects);
        Some(json!({"version":1,"projects":projects,"activity":crate::sidebar::activity_options(home)}))
    } else {
        None
    };
    Ok(json!({"data":data,"nextCursor":next,"sidebar":sidebar}))
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_names_projects_and_old_pins_survive_pagination() {
        let root = tempfile::tempdir().unwrap();
        let db = Connection::open(root.path().join("state_5.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE threads(id TEXT,title TEXT,cwd TEXT,created_at INTEGER,updated_at INTEGER,archived INTEGER,source TEXT,name TEXT,project_id TEXT,is_pinned INTEGER,section_position INTEGER,thread_section_id TEXT);
            CREATE TABLE thread_sections(id TEXT,name TEXT);
            CREATE TABLE projects(id TEXT,name TEXT,position INTEGER);
            CREATE TABLE project_roots(project_id TEXT,path TEXT,position INTEGER);
            INSERT INTO projects VALUES('project-2','Second',1),('project-1','First',0);
            INSERT INTO project_roots VALUES('project-1','/repo',0);
            INSERT INTO thread_sections VALUES('pins','Pinned');").unwrap();
        for i in 0..60 {
            db.execute("INSERT INTO threads VALUES(?1,'Original prompt','/repo',0,?2,0,'vscode',?3,NULL,0,?4,?5)",
                rusqlite::params![format!("thread-{i:02}"), i, format!("Renamed {i}"), if i == 0 { Some(2) } else { None }, if i == 0 { Some("pins") } else { None }]).unwrap();
        }
        db.execute("INSERT INTO threads SELECT 'hidden',title,cwd,0,999,1,source,name,NULL,1,0,NULL FROM threads LIMIT 1", []).unwrap();
        let state = json!({"secret":"must-not-be-returned", "thread-project-assignments":{"thread-59":{"projectKind":"local","projectId":"legacy"}},
            "app-server-project-id-by-legacy-project-id-by-host":{format!("local:{}",root.path().display()):{"legacy":"project-1"}}});
        std::fs::write(
            root.path().join(".codex-global-state.json"),
            state.to_string(),
        )
        .unwrap();
        std::fs::create_dir(root.path().join("sqlite")).unwrap();
        let automation=Connection::open(root.path().join("sqlite/codex-dev.db")).unwrap();
        automation.execute_batch("CREATE TABLE automations(id TEXT,account_id TEXT,user_id TEXT,installation_id TEXT,kind TEXT,status TEXT,target_thread_id TEXT);
            CREATE TABLE automation_runs(thread_id TEXT,automation_id TEXT,status TEXT,read_at INTEGER,created_at INTEGER);
            INSERT INTO automations VALUES('local',NULL,NULL,NULL,'heartbeat','ACTIVE','thread-59'),('other','other-account','u','i','heartbeat','ACTIVE','thread-58');
            INSERT INTO automation_runs VALUES('thread-57','local','PENDING_REVIEW',NULL,1),('thread-56','other','PENDING_REVIEW',NULL,1);").unwrap();
        let lazy = page(root.path(), 0, 20, true).unwrap();
        assert_eq!(lazy["data"].as_array().unwrap().len(), 20);
        assert_eq!(lazy["nextCursor"], "20");
        assert!(lazy["data"][0].get("_rollout").is_none());
        assert!(lazy["data"][0].get("latestReply").is_none());
        let first = list(root.path(), 0).unwrap();
        assert_eq!(first["data"][0]["hasActiveHeartbeat"],true);
        assert_eq!(first["data"][1]["hasActiveHeartbeat"],false);
        assert_eq!(first["data"][2]["isScheduled"],true);
        assert_eq!(first["data"][3]["isScheduled"],false);
        assert_eq!(first["data"].as_array().unwrap().len(), 50);
        assert_eq!(first["data"][0]["name"], "Renamed 59");
        assert_eq!(first["data"][0]["projectId"], "project-1");
        assert_eq!(first["sidebar"]["projects"][0]["name"], "First");
        assert_eq!(first["sidebar"]["projects"][1]["name"], "Second");
        assert!(!first.to_string().contains("must-not-be-returned"));
        let second = list(root.path(), 50).unwrap();
        assert_eq!(second["data"].as_array().unwrap().len(), 10);
        assert_eq!(second["data"][9]["isPinned"], true);
        assert_eq!(second["data"][9]["pinnedPosition"], 2);
        assert!(second["nextCursor"].is_null());
        // Desktop Recents uses recency, not writes from background indexing.
        db.execute_batch("ALTER TABLE threads ADD COLUMN recency_at INTEGER; ALTER TABLE threads ADD COLUMN recency_at_ms INTEGER;
            UPDATE threads SET recency_at=updated_at,recency_at_ms=updated_at*1000;
            UPDATE threads SET updated_at=9999,recency_at=1,recency_at_ms=1000 WHERE id='thread-01';
            UPDATE threads SET recency_at=100,recency_at_ms=100900 WHERE id='thread-03';
            UPDATE threads SET recency_at=100,recency_at_ms=100100 WHERE id='thread-04';").unwrap();
        let recent = list(root.path(), 0).unwrap();
        assert_eq!(recent["data"][0]["id"], "thread-03");
        assert_eq!(recent["data"][1]["id"], "thread-04");
        assert_eq!(recent["data"][0]["updatedAt"], 100.9);
    }
}
