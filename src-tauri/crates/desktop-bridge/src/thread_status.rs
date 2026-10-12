//! Bounded, passive status reads. Never activates dormant chats or returns content.
use serde_json::{json, Value};
use std::{path::Path, sync::Mutex};

fn status(state: &Value) -> &'static str {
    match state["threadRuntimeStatus"]["type"].as_str() {
        Some("active") if state["requests"].as_array().is_some_and(|r| !r.is_empty()) => "waiting",
        Some("active") => "active",
        Some("idle") => "idle",
        Some("systemError") => "error",
        _ => "unknown",
    }
}

pub fn read(home: &Path, params: &Value) -> Result<Value, String> {
    let ids = params["threadIds"].as_array().filter(|ids| ids.len() <= 32)
        .ok_or("一次最多查询 32 个会话状态")?;
    let mut threads = Vec::new();
    for id in ids {
        let id = id.as_str().ok_or("会话 ID 无效")?;
        crate::catalog::verify_thread(home, id)?;
        if !threads.contains(&id) { threads.push(id); }
    }
    let pending = Mutex::new(threads.into_iter());
    let result = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..ids.len().min(4) {
            scope.spawn(|| loop {
                let Some(id) = pending.lock().unwrap().next() else { break };
                let runtime = crate::ipc::IpcClient::passive_snapshot(home, id)
                    .map(|snapshot| status(&snapshot.state)).unwrap_or("unknown");
                result.lock().unwrap().push(json!({"id":id,"status":runtime}));
            });
        }
    });
    Ok(json!({"data":result.into_inner().unwrap(),"observedAt":crate::now_ms()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_confirmed_running_tasks_spin() {
        assert_eq!(status(&json!({"threadRuntimeStatus":{"type":"active"},"requests":[]})), "active");
        assert_eq!(status(&json!({"threadRuntimeStatus":{"type":"active"},"requests":[{}]})), "waiting");
        assert_eq!(status(&json!({"threadRuntimeStatus":{"type":"idle"}})), "idle");
        assert_eq!(status(&json!({"turns":[{"status":"inProgress"}]})), "unknown");
    }
}
