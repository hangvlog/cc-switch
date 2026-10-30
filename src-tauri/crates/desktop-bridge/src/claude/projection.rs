//! Claude Code transcript (jsonl) -> the turn/item shapes the phone renders.
//! Only user and assistant content leaves this module: other record types
//! carry the system prompt, attachments and cost state.
use serde_json::{json, Value};
use std::{collections::HashMap, fs, path::Path};

const MAX_TRANSCRIPT: u64 = 64 * 1024 * 1024;
const MAX_TEXT: usize = 8000;
const MAX_OUTPUT: usize = 4000;
const FILE_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];
const COMMAND_TOOLS: [&str; 2] = ["Bash", "PowerShell"];

pub fn clip(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.into();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…（已截断 {} 字节）", &text[..end], text.len() - end)
}

pub fn read_records(path: &Path) -> Result<Vec<Value>, String> {
    let size = fs::metadata(path).map_err(|_| "会话记录不可读")?.len();
    if size > MAX_TRANSCRIPT {
        return Err("会话记录过大，请在电脑上查看".into());
    }
    let text = fs::read_to_string(path).map_err(|_| "会话记录不可读")?;
    Ok(text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect())
}

fn conversational(record: &Value) -> bool {
    matches!(record["type"].as_str(), Some("user" | "assistant")) && record["isSidechain"] != true
}

/// The branch `--resume` continues. Two writers on one session leave the
/// other branch orphaned; it must not be shown as part of the conversation.
pub fn active_chain(records: &[Value]) -> Vec<&Value> {
    let by_uuid: HashMap<&str, &Value> = records.iter().filter_map(|r| Some((r["uuid"].as_str()?, r))).collect();
    let mut chain = std::collections::HashSet::new();
    let mut node = records.iter().rev().find(|r| conversational(r) && r["uuid"].is_string());
    while let Some(record) = node {
        let id = record["uuid"].as_str().unwrap_or_default();
        if !chain.insert(id) {
            break;
        }
        node = record["parentUuid"].as_str().and_then(|p| by_uuid.get(p).copied());
    }
    records.iter().filter(|r| r["uuid"].as_str().is_some_and(|id| chain.contains(id))).collect()
}

fn user_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.into());
    }
    let blocks = content.as_array()?;
    if blocks.iter().any(|b| b["type"] == "tool_result") {
        return None;
    }
    let text: Vec<&str> = blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect();
    (!text.is_empty()).then(|| text.join("\n"))
}

pub fn starts_turn(record: &Value) -> bool {
    record["type"] == "user" && record["isMeta"] != true && record["isSidechain"] != true
        && user_text(&record["message"]["content"]).is_some()
}

fn tool_item(block: &Value) -> Value {
    let name = block["name"].as_str().unwrap_or("");
    let input = &block["input"];
    let id = block["id"].as_str().unwrap_or("");
    if COMMAND_TOOLS.contains(&name) {
        return json!({"type":"commandExecution","id":id,"command":clip(input["command"].as_str().unwrap_or(""),MAX_OUTPUT),"status":"inProgress"});
    }
    if FILE_TOOLS.contains(&name) {
        let path = input["file_path"].as_str().or(input["notebook_path"].as_str()).unwrap_or("");
        let kind = if name == "Write" { "add" } else { "update" };
        return json!({"type":"fileChange","id":id,"changes":[{"path":path,"kind":kind}],"status":"inProgress"});
    }
    json!({"type":"dynamicToolCall","id":id,"tool":name,"arguments":clip(&input.to_string(),1000),"status":"inProgress"})
}

fn result_text(block: &Value) -> String {
    match &block["content"] {
        Value::String(text) => clip(text, MAX_OUTPUT),
        Value::Array(parts) => clip(&parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"), MAX_OUTPUT),
        _ => String::new(),
    }
}

/// Items for one user/assistant message. Tool results update `items` in place.
pub fn apply_message(items: &mut Vec<Value>, record_id: &str, role: &str, content: &Value) {
    if role == "user" {
        if let Some(text) = user_text(content) {
            items.push(json!({"type":"userMessage","id":record_id,"content":[{"type":"text","text":clip(&text,MAX_TEXT)}]}));
            return;
        }
    }
    let Some(blocks) = content.as_array() else {
        if let Some(text) = content.as_str() {
            items.push(json!({"type":"agentMessage","id":record_id,"text":clip(text,MAX_TEXT)}));
        }
        return;
    };
    for (index, block) in blocks.iter().enumerate() {
        let id = format!("{record_id}:{index}");
        match block["type"].as_str() {
            Some("text") if role == "assistant" => {
                items.push(json!({"type":"agentMessage","id":id,"text":clip(block["text"].as_str().unwrap_or(""),MAX_TEXT)}))
            }
            Some("thinking") if block["thinking"].as_str().is_some_and(|t| !t.trim().is_empty()) => {
                items.push(json!({"type":"reasoning","id":id,"summary":[clip(block["thinking"].as_str().unwrap_or(""),1500)]}))
            }
            Some("tool_use") => items.push(tool_item(block)),
            Some("tool_result") => {
                let target = block["tool_use_id"].as_str().unwrap_or("");
                if let Some(item) = items.iter_mut().rev().find(|i| i["id"] == target) {
                    item["status"] = json!(if block["is_error"] == true { "failed" } else { "completed" });
                    item["aggregatedOutput"] = json!(result_text(block));
                }
            }
            _ => {}
        }
    }
}

fn seconds(timestamp: &Value) -> Option<u64> {
    let text = timestamp.as_str()?;
    // RFC 3339 with milliseconds and a Z suffix, as written by the CLI.
    let (date, time) = text.trim_end_matches('Z').split_once('T')?;
    let mut d = date.split('-').filter_map(|v| v.parse::<i64>().ok());
    let (y, m, day) = (d.next()?, d.next()?, d.next()?);
    let mut t = time.split(':');
    let (h, min) = (t.next()?.parse::<i64>().ok()?, t.next()?.parse::<i64>().ok()?);
    let s = t.next()?.split('.').next()?.parse::<i64>().ok()?;
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let days = 365 * y + y / 4 - y / 100 + y / 400 + (153 * m + 2) / 5 + day - 1 - 719_468;
    u64::try_from(days * 86_400 + h * 3600 + min * 60 + s).ok()
}

pub struct Projection {
    pub turns: Vec<Value>,
    pub preview: String,
    pub total_turns: usize,
}

pub fn project(records: &[Value], limit: usize, running_turn: Option<&str>) -> Projection {
    let mut turns: Vec<Value> = Vec::new();
    let mut items: Vec<Value> = Vec::new();
    let mut preview = String::new();
    for record in active_chain(records) {
        if !conversational(record) {
            continue;
        }
        let id = record["uuid"].as_str().unwrap_or("");
        if starts_turn(record) {
            if let Some(turn) = turns.last_mut() {
                turn["items"] = Value::Array(std::mem::take(&mut items));
            }
            if preview.is_empty() {
                preview = clip(&user_text(&record["message"]["content"]).unwrap_or_default(), 160);
            }
            turns.push(json!({"id":id,"status":"completed","items":[],"startedAt":seconds(&record["timestamp"])}));
        }
        if turns.is_empty() {
            continue;
        }
        let role = record["message"]["role"].as_str().unwrap_or(record["type"].as_str().unwrap_or(""));
        apply_message(&mut items, id, role, &record["message"]["content"]);
        turns.last_mut().unwrap()["completedAt"] = json!(seconds(&record["timestamp"]));
    }
    if let Some(turn) = turns.last_mut() {
        turn["items"] = Value::Array(items);
        if let Some(running) = running_turn {
            turn["status"] = json!("inProgress");
            turn["clawkitTurnId"] = json!(running);
        }
    }
    let total_turns = turns.len();
    let start = total_turns.saturating_sub(limit.max(1));
    Projection { turns: turns.split_off(start), preview, total_turns }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(kind: &str, uuid: &str, parent: Option<&str>, content: Value) -> Value {
        json!({"type":kind,"uuid":uuid,"parentUuid":parent,"timestamp":"2026-10-30T08:37:17.130Z",
            "message":{"role":kind,"content":content}})
    }

    #[test]
    fn follows_the_resumed_branch_and_folds_tool_results() {
        let records = vec![
            json!({"type":"api-request-blob","message":{"role":"system","content":"SECRET SYSTEM PROMPT"}}),
            rec("user", "u1", None, json!("first question")),
            rec("assistant", "a1", Some("u1"), json!([{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"echo hi"}}])),
            rec("user", "r1", Some("a1"), json!([{"type":"tool_result","tool_use_id":"t1","content":"hi"}])),
            rec("assistant", "a2", Some("r1"), json!([{"type":"text","text":"done"}])),
            rec("user", "orphan", Some("a2"), json!("lost branch BRAVO")),
            rec("user", "u2", Some("a2"), json!("second question")),
            rec("assistant", "a3", Some("u2"), json!([{"type":"text","text":"ALPHA"}])),
        ];
        let projection = project(&records, 20, None);
        let text = serde_json::to_string(&projection.turns).unwrap();
        assert_eq!(projection.total_turns, 2);
        assert!(!text.contains("BRAVO") && !text.contains("SECRET"));
        let command = &projection.turns[0]["items"][1];
        assert_eq!(command["type"], "commandExecution");
        assert_eq!(command["status"], "completed");
        assert_eq!(command["aggregatedOutput"], "hi");
        assert_eq!(projection.turns[0]["startedAt"], 1_793_349_437u64);
        assert_eq!(projection.preview, "first question");
    }

    #[test]
    fn marks_running_turn_and_limits_turns() {
        let records = vec![
            rec("user", "u1", None, json!("one")),
            rec("user", "u2", Some("u1"), json!("two")),
        ];
        let projection = project(&records, 1, Some("turn-x"));
        assert_eq!(projection.turns.len(), 1);
        assert_eq!(projection.turns[0]["status"], "inProgress");
        assert_eq!(projection.turns[0]["items"][0]["content"][0]["text"], "two");
    }

    #[test]
    fn clips_on_char_boundaries() {
        let text = "汉".repeat(5000);
        assert!(clip(&text, 10).starts_with("汉汉汉"));
    }
}
