use serde_json::{json, Value};
use std::collections::HashSet;

pub fn turns(state: &Value) -> Vec<&Value> {
    if state["turnHistory"]["kind"] == "canonical" {
        let history = &state["turnHistory"]["history"];
        return history["islands"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|island| island["entries"].as_array().into_iter().flatten())
            .filter_map(|entry| entry["value"].as_str())
            .filter_map(|key| history["entitiesByKey"].get(key))
            .collect();
    }
    state["turns"].as_array().into_iter().flatten().collect()
}

pub fn conversation_block_reason(state: &Value) -> Option<&'static str> {
    if state["resumeState"] != "resumed" {
        return Some("请先在电脑打开并加载原任务");
    }
    if state["source"].is_object() || !state["parentThreadId"].is_null() {
        return Some("子代理任务不能从手机直接发送");
    }
    if state["ephemeral"] == true || state["sideConversation"] == true {
        return Some("临时任务不支持手机接续");
    }
    if state["environments"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item["environmentId"]
                .as_str()
                .is_some_and(|id| id != "local")
        })
    }) {
        return Some("此版本仅支持本机原任务");
    }
    None
}

pub fn send_block_reason(state: &Value) -> Option<&'static str> {
    if let Some(reason) = conversation_block_reason(state) {
        return Some(reason);
    }
    if state["requests"]
        .as_array()
        .is_none_or(|requests| !requests.is_empty())
    {
        return Some("任务正在等待审批或输入，请先在电脑处理");
    }
    let status = state["threadRuntimeStatus"]["type"]
        .as_str()
        .unwrap_or("unknown");
    if !matches!(status, "idle" | "systemError")
        || turns(state)
            .last()
            .is_some_and(|turn| turn["status"] == "inProgress")
    {
        return Some("任务正在运行或状态未确认，请等待并刷新");
    }
    None
}

// Steering stays on the original owner and never interrupts its active turn.
pub fn can_steer(state: &Value) -> bool {
    conversation_block_reason(state).is_none()
        && state["requests"].as_array().is_some_and(Vec::is_empty)
        && state["threadRuntimeStatus"]["type"] == "active"
        && turns(state)
            .last()
            .is_some_and(|t| t["status"] == "inProgress")
}

pub fn dispatch_block_reason(state: &Value) -> Option<&'static str> {
    if can_steer(state) {
        None
    } else {
        send_block_reason(state)
    }
}

pub fn contains_request(state: &Value, request: &str) -> bool {
    turns(state).iter().any(|turn| {
        turn["items"].as_array().into_iter().flatten().any(|item| {
            (item["type"] == "userMessage" && item["clientId"] == request)
                || (item["type"] == "steeringUserMessage"
                    && item["status"] == "accepted"
                    && item["serverUserMessageId"].is_string()
                    && item["clientUserMessageId"] == request)
        })
    })
}

fn bounded_text(value: &str, budget: &mut usize) -> String {
    let text: String = value.chars().take(24_000.min(*budget)).collect();
    *budget -= text.chars().count();
    text
}

pub fn visible_thread(
    state: &Value,
    owner: &str,
    revision: u64,
    attachments: &crate::attachments::Attachments,
) -> Value {
    let mut budget = 160_000;
    let mut count = 0;
    let all = turns(state);
    let recent = &all[all.len().saturating_sub(30)..];
    let mut seen = HashSet::new();
    // Newest first while enforcing the limit; restore chronological order afterward.
    let mut output = Vec::new();
    for turn in recent.iter().rev() {
        let mut items = Vec::new();
        for item in turn["items"].as_array().into_iter().flatten().rev() {
            if count >= 80 || budget == 0 {
                break;
            }
            let kind = item["type"].as_str().unwrap_or("");
            if !matches!(kind, "userMessage" | "steeringUserMessage" | "agentMessage") {
                if let Some(projected) = crate::owner_output::project(item, attachments, &mut budget) {
                    if seen.insert(projected["id"].as_str().unwrap_or_default().to_owned()) {
                        items.push(projected); count += 1;
                    }
                }
                continue;
            }
            if kind == "agentMessage"
                && (item["phase"] == "analysis" || item["channel"] == "analysis")
            {
                continue;
            }
            if kind == "steeringUserMessage"
                && item["status"]
                    .as_str()
                    .is_some_and(|status| status != "accepted")
            {
                continue;
            }
            // Transcript history uses the server item ID. Client IDs are only
            // aliases for the send receipt and accepted steering placeholder.
            let client_id = item["clientId"]
                .as_str()
                .or_else(|| item["clientUserMessageId"].as_str());
            let key = item["serverUserMessageId"]
                .as_str()
                .or_else(|| item["id"].as_str())
                .or(client_id)
                .unwrap_or("");
            if !key.is_empty() && !seen.insert(key.to_owned()) {
                continue;
            }
            let raw = if kind == "agentMessage" {
                item["text"].as_str().unwrap_or("").to_owned()
            } else {
                item.get("content")
                    .or_else(|| item.get("input"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|part| match part["type"].as_str() {
                        Some("text") => part["text"].as_str(),
                        Some("image" | "localImage" | "local_image" | "input_image") => {
                            Some("[图片]")
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            if raw.is_empty() {
                continue;
            }
            let text = bounded_text(&raw, &mut budget);
            items.push(if kind == "agentMessage" {
                json!({"id":key,"type":"agentMessage","text":text,"phase":item["phase"]})
            } else {
                json!({"id":key,"clientId":client_id,"type":"userMessage","content":attachments.project_content(item.get("content").or_else(||item.get("input")).unwrap_or(&Value::Null))})
            });
            count += 1;
        }
        items.reverse();
        if !items.is_empty() {
            output.push(json!({"id":turn["turnId"],"status":turn["status"],"items":items}));
        }
    }
    output.reverse();
    json!({"id":state["id"],"preview":state["title"],"name":state["title"],"cwd":state["cwd"],
        "createdAt":state["createdAt"],"updatedAt":state["updatedAt"],"turns":output,
        "desktop":{"owner":owner,"revision":revision,"source":"live","observedAt":crate::now_ms(),
            "status":crate::thread_status::owner_status(state),"model":state["latestModel"],
            "approvals":crate::owner_control::approvals(state,owner),
            "activeTurnId":all.last().filter(|t| t["status"] == "inProgress").map(|t| &t["turnId"]),
            "canSteer":can_steer(state),"clarifications":crate::clarification::pending(state),"canQueue":conversation_block_reason(state).is_none(),"canSend":send_block_reason(state).is_none(),"reason":send_block_reason(state)}})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn optimistic_user_messages_do_not_confirm_acceptance() {
        let mut state = json!({"turns":[{"params":{"clientUserMessageId":"r"},"items":[
            {"type":"steeringUserMessage","id":"r","clientUserMessageId":"r","status":"pending"}]}]});
        assert!(!contains_request(&state, "r"));
        state["turns"][0]["items"] =
            json!([{"type":"userMessage","clientId":"r","id":"server-id"}]);
        assert!(contains_request(&state, "r"));
    }
    #[test]
    fn excludes_internal_content_and_preserves_visible_messages() {
        let state = json!({"id":"t","resumeState":"resumed","requests":[],"threadRuntimeStatus":{"type":"idle"},
            "secret":"must-not-leak","turns":[{"turnId":"turn","status":"completed","items":[
                {"type":"userMessage","id":"u","content":[{"type":"text","text":"hello"}]},
                {"type":"agentMessage","id":"hidden","phase":"analysis","text":"private reasoning"},
                {"type":"commandExecution","aggregatedOutput":"secret tool output"},
                {"type":"agentMessage","id":"a","text":"world"}]}]});
        let root = tempfile::tempdir().unwrap();
        let attachments = crate::attachments::Attachments::open(root.path()).unwrap();
        let result = visible_thread(&state, "owner", 1, &attachments);
        assert_eq!(result["turns"][0]["items"].as_array().unwrap().len(), 2);
        assert!(!result.to_string().contains("secret"));
        assert!(!result.to_string().contains("private reasoning"));
        assert_eq!(result["desktop"]["canSend"], true);
    }
    #[test]
    fn history_and_live_share_server_ids_and_keep_receipt_aliases() {
        let state = json!({"turns":[{"turnId":"turn","items":[
            {"type":"userMessage","id":"server-1","clientId":"client-1","content":[{"type":"text","text":"same"}]},
            {"type":"steeringUserMessage","id":"client-1","clientUserMessageId":"client-1","serverUserMessageId":"server-1","status":"accepted","input":[{"type":"text","text":"same"}]},
            {"type":"userMessage","id":"server-2","clientId":"client-2","content":[{"type":"text","text":"same"}]}
        ]}]});
        let root = tempfile::tempdir().unwrap();
        let attachments = crate::attachments::Attachments::open(root.path()).unwrap();
        let result = visible_thread(&state, "owner", 1, &attachments);
        let items = result["turns"][0]["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["id"], "server-1");
        assert_eq!(items[0]["clientId"], "client-1");
        assert_eq!(items[1]["id"], "server-2");
        assert!(contains_request(&state, "client-1"));
    }
    #[test]
    fn refuses_busy_unloaded_or_approval_states() {
        let mut state =
            json!({"resumeState":"resumed","requests":[],"threadRuntimeStatus":{"type":"idle"}});
        assert!(send_block_reason(&state).is_none());
        state["threadRuntimeStatus"]["type"] = json!("active");
        assert!(send_block_reason(&state).is_some());
        state["threadRuntimeStatus"]["type"] = json!("idle");
        state["requests"] = json!([{"method":"approve"}]);
        assert!(send_block_reason(&state).is_some());
    }

    #[test]
    fn ordinary_owner_keeps_tool_activity_without_execution_details() {
        let state = json!({"id":"t","resumeState":"resumed","requests":[],"threadRuntimeStatus":{"type":"idle"},
            "turns":[{"turnId":"turn","status":"completed","items":[
                {"type":"commandExecution","id":"approval8","command":"printf OWNER_APPROVAL_OK",
                 "status":"completed","aggregatedOutput":"OWNER_APPROVAL_OK","exitCode":0,"privateMetadata":"omit"}]}]});
        let root = tempfile::tempdir().unwrap();
        let attachments = crate::attachments::Attachments::open(root.path()).unwrap();
        let result = visible_thread(&state,"owner",1,&attachments);
        assert!(result["turns"][0]["items"][0].get("aggregatedOutput").is_none());
        assert_eq!(result["turns"][0]["items"][0]["type"],"toolActivity");
        assert_eq!(result["turns"][0]["items"][0]["status"],"completed");
        assert!(result["turns"][0]["items"][0].get("privateMetadata").is_none());
    }
}
