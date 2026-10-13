//! Controls sent only to a freshly discovered original owner. Never retries writes.
use crate::{history, ipc::{IpcClient, Snapshot}};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

fn approval_method(request: &Value) -> Option<(&'static str, &'static str)> {
    match request["method"].as_str()? {
        "item/commandExecution/requestApproval" => Some(("thread-follower-command-approval-decision", "decision")),
        "item/fileChange/requestApproval" => Some(("thread-follower-file-approval-decision", "decision")),
        "item/permissions/requestApproval" => Some(("thread-follower-permissions-request-approval-response", "response")),
        "mcpServer/elicitation/request" if crate::native::responses::remote_form(request) =>
            Some(("thread-follower-submit-mcp-server-elicitation-response", "response")),
        _ => None,
    }
}

fn token(thread: &str, owner: &str, request: &Value) -> String {
    format!("{:x}", Sha256::digest(json!([thread, owner, request]).to_string().as_bytes()))
}

pub fn approvals(state: &Value, owner: &str) -> Vec<Value> {
    let Some(thread) = state["id"].as_str() else { return vec![] };
    if history::conversation_block_reason(state).is_some() { return vec![]; }
    state["requests"].as_array().into_iter().flatten()
        .filter(|r| approval_method(r).is_some() && r["params"]["threadId"] == thread)
        .map(|r| json!({"requestId":r["id"],"method":r["method"],"params":r["params"],
            "owner":owner,"requestToken":token(thread,owner,r)})).collect()
}

fn current_request<'a>(snapshot: &'a Snapshot, params: &Value) -> Result<&'a Value, String> {
    if history::conversation_block_reason(&snapshot.state).is_some() {
        return Err("原任务状态不可用，请刷新".into());
    }
    let thread = params["threadId"].as_str().ok_or("缺少任务编号")?;
    let request = snapshot.state["requests"].as_array().into_iter().flatten()
        .find(|r| r["id"] == params["serverRequestId"] && r["params"]["threadId"] == thread)
        .ok_or("此审批已被处理，请刷新任务")?;
    if params["owner"] != snapshot.owner || params["requestToken"] != token(thread,&snapshot.owner,request) {
        return Err("审批内容或任务 owner 已变化，请重新查看后决定".into());
    }
    Ok(request)
}

pub fn respond(home: &Path, thread: &str, params: &Value) -> Result<Value, String> {
    let owner = params["owner"].as_str().filter(|s| !s.is_empty()).ok_or("缺少审批 owner")?;
    let mut client = IpcClient::connect(home)?;
    let snapshot = client.snapshot(thread, Some(owner))?;
    let request = current_request(&snapshot,params)?;
    let (method, field) = approval_method(request).ok_or("此请求需要在电脑处理")?;
    let response = crate::native::responses::build(request,params)?;
    let mut payload = json!({"conversationId":thread,"requestId":request["id"]});
    payload[field] = if field == "decision" {response["decision"].clone()} else {response};
    let result = client.request(method,payload,1,Some(owner))?;
    if result["result"]["ok"] != true { return Err("电脑未确认审批提交，请刷新核对".into()); }
    let fresh = IpcClient::connect(home)?.snapshot(thread,Some(owner))?;
    if fresh.state["requests"].as_array().is_none_or(|rows|rows.iter().any(|r|r["id"] == request["id"])) {
        return Err("审批仍待处理，请刷新核对；不会自动重试".into());
    }
    Ok(json!({"status":"resolved","serverRequestId":request["id"]}))
}

fn validate_turn(state: &Value, turn: &str) -> Result<(), String> {
    if history::conversation_block_reason(state).is_some() || !history::turns(state).last()
        .is_some_and(|t|t["turnId"] == turn && t["status"] == "inProgress") {
        return Err("执行轮次已变化，请刷新任务后再停止".into());
    }
    Ok(())
}

pub fn interrupt(home: &Path, thread: &str, turn: &str) -> Result<Value, String> {
    let mut client = IpcClient::connect(home)?;
    let snapshot = client.snapshot(thread,None)?;
    validate_turn(&snapshot.state,turn)?;
    // Version 4 checks expectedTurnId again inside the owner immediately before interrupting.
    let result = client.request("thread-follower-interrupt-turn",
        json!({"conversationId":thread,"mode":"system","expectedTurnId":turn}),4,Some(&snapshot.owner))?;
    if result["result"]["ok"] != true || result["result"]["interruptedTurnId"] != turn {
        return Err("该轮次已结束或变化，未停止新的任务".into());
    }
    Ok(json!({"interruptedTurnId":turn}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> Snapshot {
        Snapshot {owner:"owner".into(),revision:4,state:json!({"id":"t","resumeState":"resumed",
            "requests":[{"id":0,"method":"item/commandExecution/requestApproval","params":{"threadId":"t","command":"printf test","availableDecisions":["cancel"]}}],
            "turns":[{"turnId":"new-turn","status":"inProgress"}]})}
    }
    #[test]
    fn binds_decision_to_owner_and_exact_pending_content() {
        let mut s=snapshot();let rows=approvals(&s.state,&s.owner);
        assert_eq!(rows.len(),1);
        let mut p=json!({"threadId":"t","owner":"owner","serverRequestId":0,"requestToken":rows[0]["requestToken"]});
        assert!(current_request(&s,&p).is_ok());
        p["owner"]=json!("old-owner");assert!(current_request(&s,&p).is_err());p["owner"]=json!("owner");
        s.state["requests"][0]["params"]["command"]=json!("different command");assert!(current_request(&s,&p).is_err());
        s.state["requests"]=json!([]);assert!(current_request(&s,&p).is_err());
    }
    #[test]
    fn stale_or_completed_turn_cannot_interrupt_current_turn() {
        let mut s=snapshot();assert!(validate_turn(&s.state,"new-turn").is_ok());
        assert!(validate_turn(&s.state,"old-turn").is_err());
        s.state["turns"][0]["status"]=json!("completed");assert!(validate_turn(&s.state,"new-turn").is_err());
    }
}
