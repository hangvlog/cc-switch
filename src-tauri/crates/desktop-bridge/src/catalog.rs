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
    let sql = format!("SELECT t.id,t.title,t.cwd,t.created_at,t.updated_at,{extra} FROM threads t WHERE t.archived=0 AND t.source NOT LIKE '%subAgent%' ORDER BY t.updated_at DESC,t.id LIMIT 51 OFFSET ?1");
    let mut query = connection
        .prepare(&sql)
        .map_err(|_| "Codex 任务索引结构已变化")?;
    let rows = query.query_map([offset as i64],|row| Ok(json!({"id":row.get::<_,String>(0)?,
        "preview":row.get::<_,String>(1)?,"cwd":row.get::<_,String>(2)?,"createdAt":row.get::<_,i64>(3)?,
        "updatedAt":row.get::<_,i64>(4)?,"name":row.get::<_,Option<String>>(5)?,
        "projectId":row.get::<_,Option<String>>(6)?,"isPinned":row.get::<_,bool>(7)?,
        "pinnedPosition":row.get::<_,Option<i64>>(8)?,"turns":[],"desktop":{"source":"catalog","status":"unknown","canSend":false}})))
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
    let sidebar = if modern {
        let projects = crate::sidebar::projects(&connection)?;
        crate::sidebar::assign_projects(home, &mut data, &projects);
        Some(json!({"version":1,"projects":projects}))
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
        let first = list(root.path(), 0).unwrap();
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
    }
}
