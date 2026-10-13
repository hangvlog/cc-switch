//! Read-only projection of the installed desktop's automation visibility scope.
//! Never expose automation prompts, principal claims, or credentials to the phone.
use base64::Engine;
use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value;
use std::{collections::HashSet, io::Read, path::Path};

fn json_file(path: &Path) -> Option<Value> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() > 8 * 1024 * 1024 { return None; }
    serde_json::from_slice(&bytes).ok()
}

// Mirrors SF/wF: legacy rows are visible before migration and during migration.
fn scope(home: &Path) -> (bool, Option<String>, Option<String>, Option<String>) {
    let state = json_file(&home.join(".codex-global-state.json")).unwrap_or(Value::Null);
    let Some(installation) = state["electron-automations-installation-id"].as_str() else {
        return (true, None, None, None);
    };
    let auth = json_file(&home.join("auth.json")).unwrap_or(Value::Null);
    let principal = auth["tokens"]["access_token"].as_str().and_then(|token| token.split('.').nth(1))
        .and_then(|part| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(part).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok()).unwrap_or(Value::Null);
    let claims = &principal["https://api.openai.com/auth"];
    let account = claims["chatgpt_account_id"].as_str().or_else(|| claims["account_id"].as_str());
    let user = claims["user_id"].as_str().or_else(|| claims["chatgpt_user_id"].as_str());
    let (Some(account), Some(user)) = (account, user) else { return (true, None, None, None) };
    let states = &state["electron-automations-habitat-migration-state"];
    let status = states[format!("{account}:{user}:{installation}")].as_str()
        .or_else(|| states[format!("{account}:{user}")].as_str());
    if !matches!(status, Some("in_progress" | "complete")) { return (true, None, None, None); }
    (status != Some("complete"), Some(account.into()), Some(user.into()), Some(installation.into()))
}

pub fn annotate(home: &Path, threads: &mut [Value]) {
    let Ok(db) = Connection::open_with_flags(home.join("sqlite/codex-dev.db"), OpenFlags::SQLITE_OPEN_READ_ONLY) else { return };
    let _ = db.busy_timeout(std::time::Duration::from_millis(300));
    let (legacy, account, user, installation) = scope(home);
    let filter = "((?1=1 AND a.account_id IS NULL AND a.user_id IS NULL AND a.installation_id IS NULL) OR (a.account_id=?2 AND a.user_id=?3 AND a.installation_id=?4))";
    let read = |sql: String| -> Option<HashSet<String>> {
        let mut statement = db.prepare(&sql).ok()?;
        let rows = statement.query_map(params![legacy, account, user, installation], |r| r.get::<_, String>(0)).ok()?;
        rows.collect::<Result<HashSet<_>, _>>().ok()
    };
    // Same 200-row inbox window plus unread runs used by adr/ycr in the renderer.
    let scheduled = read(format!("SELECT thread_id FROM (SELECT r.thread_id FROM automation_runs r JOIN automations a ON a.id=r.automation_id WHERE {filter} AND r.status IN ('IN_PROGRESS','PENDING_REVIEW','ACCEPTED','ARCHIVED') ORDER BY r.status='IN_PROGRESS' DESC,r.status='PENDING_REVIEW' DESC,r.created_at DESC LIMIT 200) UNION SELECT r.thread_id FROM automation_runs r JOIN automations a ON a.id=r.automation_id WHERE {filter} AND r.read_at IS NULL AND r.status IN ('PENDING_REVIEW','ACCEPTED')"));
    let heartbeat = read(format!("SELECT a.target_thread_id FROM automations a WHERE {filter} AND a.kind='heartbeat' AND a.status='ACTIVE' AND a.target_thread_id IS NOT NULL"));
    for thread in threads {
        let Some(id) = thread["id"].as_str().map(str::to_owned) else { continue };
        if let Some(ids) = &scheduled { thread["isScheduled"] = Value::Bool(ids.contains(&id)); }
        if let Some(ids) = &heartbeat { thread["hasActiveHeartbeat"] = Value::Bool(ids.contains(&id)); }
    }
}
