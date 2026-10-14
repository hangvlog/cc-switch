//! Resolve only the notification's display fields; never load transcript or runtime state.
use serde_json::{json, Value};
use std::path::Path;

fn clean(value: &Value) -> String {
    value.as_str().unwrap_or_default().split_whitespace().collect::<Vec<_>>()
        .join(" ").chars().filter(|c| !c.is_control()).take(160).collect()
}

pub(crate) fn enrich(home: &Path, result: &mut Value) -> Result<(), String> {
    let Some(events) = result["events"].as_array_mut() else { return Ok(()) };
    if events.is_empty() { return Ok(()) }
    let db = crate::catalog::connection(home)?;
    let mut columns = db.prepare("PRAGMA table_info(threads)").map_err(|e| e.to_string())?;
    let columns = columns.query_map([], |r| r.get::<_, String>(1)).map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    let field = |name: &str| if columns.iter().any(|v| v == name) { name.to_owned() } else { "NULL".into() };
    let sql = format!("SELECT id,title,cwd,{},{} FROM threads WHERE id=?1",field("name"),field("project_id"));
    let mut query = db.prepare(&sql).map_err(|e| e.to_string())?;
    let projects = crate::sidebar::projects(&db).unwrap_or_default();
    let mut rows = Vec::new();
    for event in events.iter().take(100) {
        let Some(id) = event["threadId"].as_str() else { continue };
        if rows.iter().any(|v: &Value| v["id"] == id) { continue }
        if let Ok(row) = query.query_row([id], |r| Ok(json!({"id":r.get::<_,String>(0)?,
            "title":r.get::<_,Option<String>>(1)?,"cwd":r.get::<_,Option<String>>(2)?,
            "name":r.get::<_,Option<String>>(3)?,"projectId":r.get::<_,Option<String>>(4)?}))) { rows.push(row); }
    }
    crate::sidebar::assign_projects(home,&mut rows,&projects);
    for event in events {
        let Some(row) = rows.iter().find(|r| r["id"] == event["threadId"]) else { continue };
        let mut title = clean(&row["name"]);
        if title.is_empty() { title = clean(&row["title"]); }
        if title.is_empty() { title = "未命名会话".into(); }
        let mut project = projects.iter().find(|p| p["id"] == row["projectId"])
            .map(|p| clean(&p["name"])).unwrap_or_default();
        if project.is_empty() && !row["projectId"].is_null() { project = "项目暂未同步".into(); }
        if project.is_empty() { project = "未归属项目".into(); }
        event["threadTitle"] = json!(title);
        event["projectName"] = json!(project);
    }
    Ok(())
}
