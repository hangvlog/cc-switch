use rusqlite::Connection;
use serde_json::{json, Value};
use std::io::Read;
use std::path::Path;

/// Read only the desktop's project assignments. Never send the global state file.
pub fn assign_projects(home: &Path, threads: &mut [Value], projects: &[Value]) {
    let state = std::fs::File::open(home.join(".codex-global-state.json"))
        .ok()
        .and_then(|file| {
            let mut bytes = Vec::new();
            file.take(8 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .ok()?;
            if bytes.len() > 8 * 1024 * 1024 {
                return None;
            }
            serde_json::from_slice::<Value>(&bytes).ok()
        });
    let Some(state) = state else { return };
    let host = format!("local:{}", home.display());
    let migrated = &state["app-server-project-id-by-legacy-project-id-by-host"][&host];
    for thread in threads {
        if thread["projectId"].is_string() {
            continue;
        }
        let Some(id) = thread["id"].as_str() else {
            continue;
        };
        let assignment = &state["thread-project-assignments"][id];
        if assignment["projectKind"] != "local" {
            continue;
        }
        let Some(legacy) = assignment["projectId"].as_str() else {
            continue;
        };
        let project = migrated[legacy].as_str().unwrap_or(legacy);
        if projects.iter().any(|p| p["id"] == project) {
            thread["projectId"] = json!(project);
        }
    }
}

pub fn projects(connection: &Connection) -> Result<Vec<Value>, String> {
    let mut query = connection
        .prepare("SELECT id,name FROM projects ORDER BY position,id")
        .map_err(|e| e.to_string())?;
    let rows = query
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;
    let mut projects = Vec::new();
    for row in rows {
        let (id, name) = row.map_err(|e| e.to_string())?;
        let mut roots = connection
            .prepare("SELECT path FROM project_roots WHERE project_id=?1 ORDER BY position")
            .map_err(|e| e.to_string())?;
        let paths = roots
            .query_map([&id], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        projects.push(json!({"id":id,"name":name,"roots":paths}));
    }
    Ok(projects)
}
