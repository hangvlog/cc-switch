//! Enrich bounded reminder metadata and reconcile resolution with the official owner.
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
    crate::unread::annotate(home,&mut rows);
    for event in events.iter_mut() {
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
        if event["timestampMs"].is_null() {
            if let Some(timestamp)=event["timestamp"].as_str() {
                if let Ok(ms)=db.query_row("SELECT CAST((julianday(?1)-2440587.5)*86400000 AS INTEGER)",[timestamp],|r|r.get::<_,u64>(0)) {
                    event["timestampMs"]=json!(ms);
                }
            }
        }
        if event["kind"]=="completed" && row["hasUnreadTurn"]==false {
            // Give the desktop time to publish its unread transition after completion.
            if event["timestampMs"].as_u64().is_some_and(|t|crate::now_ms().saturating_sub(t)>10_000) {
                event["resolved"]=json!(true);
            }
        }
    }
    let threads:Vec<String> = events.iter().filter(|e|e["kind"]=="attention" && e["resolved"]!=true && e["pending"]!=true)
        .filter_map(|e|e["threadId"].as_str()).map(str::to_owned)
        .collect::<std::collections::HashSet<_>>().into_iter().collect();
    // One bounded passive batch; never opens a chat or waits once per notification.
    let _=crate::ipc::IpcClient::passive_batch(home,&threads.iter().map(String::as_str).collect::<Vec<_>>(),|thread,state| {
        if state["resumeState"]!="resumed" || !state["requests"].is_array() { return; }
        let questions=crate::clarification::pending(state);
        let requests=state["requests"].as_array();
        for event in events.iter_mut().filter(|e|e["threadId"]==thread && e["kind"]=="attention" && e["resolved"]!=true && e["pending"]!=true) {
            let pending=if let Some(id)=event["requestId"].as_str() {
                questions.iter().any(|q|q["requestId"]==id || q["requestId"]==format!("async:{id}"))
                    || requests.into_iter().flatten().any(|q|q["id"]==id)
            } else { !questions.is_empty() || requests.is_some_and(|r|!r.is_empty()) };
            if !pending && event["timestampMs"].as_u64().is_some_and(|t|crate::now_ms().saturating_sub(t)>10_000) { event["resolved"]=json!(true); }
        }
    });
    Ok(())
}
