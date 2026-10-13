//! Passive observations from the original owner; never infer activity from transcript tails.
use serde_json::{json, Value};
use std::{path::Path, sync::Mutex, collections::HashMap};

pub(crate) fn runtime_status(runtime: &Value) -> &'static str {
    match runtime["type"].as_str() {
        Some("active") if runtime["activeFlags"].as_array().is_some_and(|flags|
            flags.iter().any(|f| f == "waitingOnApproval" || f == "waitingOnUserInput")) => "waiting",
        Some("active") => "active",
        Some("idle") => "idle",
        Some("systemError") => "error",
        _ => "unknown",
    }
}

pub(crate) fn owner_status(state: &Value) -> &'static str {
    if state["requests"].as_array().is_some_and(|r| !r.is_empty()) || !crate::clarification::pending(state).is_empty() {
        "waiting"
    } else { runtime_status(&state["threadRuntimeStatus"]) }
}

fn project(state: &Value) -> Value {
    let all = crate::history::turns(state);
    let latest = all.last();
    let status = owner_status(state);
    let silent = latest.is_some_and(|t| t["status"] == "completed" && t["automationKind"] == "heartbeat"
        && t["automationNotificationDecision"] == "DONT_NOTIFY");
    let heartbeat = latest.filter(|t| t["automationKind"] == "heartbeat" &&
        (t["status"] == "inProgress" || t["status"] == "completed" && t["automationNotificationDecision"].is_null()))
        .and_then(|t| t["turnStartedAtMs"].as_u64());
    json!({"id":state["id"],"status":status,"hasUnreadTurn":state["hasUnreadTurn"],
        "latestReply":crate::activity::live_summary(state),"source":"owner","observedAt":crate::now_ms(),
        "silentHeartbeat":silent,"pendingHeartbeatStartedAtMs":heartbeat})
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
    let mut observations=HashMap::new();
    let _=crate::ipc::IpcClient::passive_batch(home,&threads,|id,state| {observations.insert(id.to_owned(),project(state));});
    let native = crate::native::event_ready(home);
    let pending = Mutex::new(threads.into_iter());
    let result = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..ids.len().min(4) {
            scope.spawn(|| loop {
                let Some(id) = pending.lock().unwrap().next() else { break };
                // Renderer owns async questions and visible messages; prefer its snapshot.
                let value = match observations.get(id) {
                    Some(value) => value.clone(),
                    None if native => crate::native::request(home,"thread/read",json!({"threadId":id,"includeTurns":false}))
                        .map(|v|json!({"id":id,"status":runtime_status(&v["thread"]["status"]),"source":"engine","observedAt":crate::now_ms()}))
                        .unwrap_or_else(|_|json!({"id":id,"status":"unknown"})),
                    None => json!({"id":id,"status":"unknown"}),
                };
                result.lock().unwrap().push(value);
            });
        }
    });
    let mut data=result.into_inner().unwrap();
    // Only catalog/engine fallbacks need the persisted read bucket. A fresh owner
    // snapshot must not be overwritten by a potentially lagging disk write.
    let mut fallback:Vec<_>=data.iter().filter(|r| r["source"]!="owner").cloned().collect();
    crate::unread::annotate(home,&mut fallback);
    for row in &mut data { if row["source"]!="owner" { if let Some(f)=fallback.iter().find(|f| f["id"]==row["id"]) {*row=f.clone();} } }
    Ok(json!({"data":data,"observedAt":crate::now_ms()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_confirmed_running_tasks_spin() {
        let mut s=json!({"id":"t","threadRuntimeStatus":{"type":"active","activeFlags":[]},"requests":[],"turns":[]});
        assert_eq!(project(&s)["status"],"active");
        s["threadRuntimeStatus"]["activeFlags"]=json!(["waitingOnUserInput"]);
        assert_eq!(project(&s)["status"],"waiting");
        s["threadRuntimeStatus"]=json!({"type":"idle"});
        assert_eq!(project(&s)["status"],"idle");
        s["turns"]=json!([{"turnId":"u","status":"inProgress","items":[{"type":"agentMessage","id":"a","text":"Visible", "questions":[{"title":"Choose"}]}]}]);
        assert_eq!(project(&s)["status"],"waiting");
        assert_eq!(project(&s)["latestReply"],"Visible");
        s["turns"][0]=json!({"status":"completed","automationKind":"heartbeat","automationNotificationDecision":"DONT_NOTIFY"});
        assert_eq!(project(&s)["silentHeartbeat"],true);
        assert_eq!(runtime_status(&Value::Null),"unknown");
        assert_eq!(runtime_status(&json!({"type":"notLoaded"})),"unknown");
    }
}
