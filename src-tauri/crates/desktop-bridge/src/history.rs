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

pub fn send_block_reason(state: &Value) -> Option<&'static str> {
    if state["resumeState"] != "resumed" {
        return Some("请先在电脑打开并加载原任务");
    }
    if state["requests"]
        .as_array()
        .is_none_or(|requests| !requests.is_empty())
    {
        return Some("任务正在等待审批或输入，请先在电脑处理");
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

pub fn visible_thread(state: &Value, owner: &str, revision: u64) -> Value {
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
            let key = item["clientId"]
                .as_str()
                .or_else(|| item["clientUserMessageId"].as_str())
                .or_else(|| item["id"].as_str())
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
                    .filter(|part| part["type"] == "text")
                    .filter_map(|part| part["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            if raw.is_empty() {
                continue;
            }
            let text = bounded_text(&raw, &mut budget);
            items.push(if kind == "agentMessage" {
                json!({"id":key,"type":"agentMessage","text":text})
            } else {
                json!({"id":key,"type":"userMessage","content":[{"type":"text","text":text}]})
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
            "status":state["threadRuntimeStatus"]["type"],"model":state["latestModel"],
            "canSend":send_block_reason(state).is_none(),"reason":send_block_reason(state)}})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn excludes_internal_content_and_preserves_visible_messages() {
        let state = json!({"id":"t","resumeState":"resumed","requests":[],"threadRuntimeStatus":{"type":"idle"},
            "secret":"must-not-leak","turns":[{"turnId":"turn","status":"completed","items":[
                {"type":"userMessage","id":"u","content":[{"type":"text","text":"hello"}]},
                {"type":"agentMessage","id":"hidden","phase":"analysis","text":"private reasoning"},
                {"type":"commandExecution","aggregatedOutput":"secret tool output"},
                {"type":"agentMessage","id":"a","text":"world"}]}]});
        let result = visible_thread(&state, "owner", 1);
        assert_eq!(result["turns"][0]["items"].as_array().unwrap().len(), 2);
        assert!(!result.to_string().contains("secret"));
        assert!(!result.to_string().contains("private reasoning"));
        assert_eq!(result["desktop"]["canSend"], true);
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
}
