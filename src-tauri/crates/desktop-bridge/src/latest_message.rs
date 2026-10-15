//! Keep current runtime/interaction state while deferring older visible messages.
use serde_json::{json, Value};

pub fn compact(thread: &mut Value) {
    let Some(turns) = thread["turns"].as_array() else { return };
    let latest = turns.iter().enumerate().rev().find_map(|(index, turn)| {
        turn["items"].as_array()?.iter().rev().find(|item| {
            matches!(item["type"].as_str(), Some("agentMessage" | "userMessage"))
                && item["phase"] != "analysis" && item["channel"] != "analysis"
        }).map(|item| (index, item.clone()))
    });
    let output: Vec<_> = turns.iter().enumerate().filter_map(|(index, turn)| {
        let mut result = turn.clone();
        if let Some((_, item)) = latest.as_ref().filter(|(i, _)| *i == index) {
            result["items"] = json!([item]);
        } else if turn["status"] == "inProgress" {
            result["items"] = json!([]);
        } else { return None; }
        Some(result)
    }).collect();
    thread["turns"] = json!(output);
    // Saved history supplies a message-level cursor, including earlier messages
    // in the same turn. An engine turn cursor alone would skip those messages.
    thread["historyCursor"] = json!("latest");
    thread["latestOnly"] = json!(true);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn latest_text_preserves_active_turn_and_interactions() {
        let mut thread=json!({"turns":[
            {"id":"old","status":"completed","items":[{"id":"u","type":"userMessage"},{"id":"a","type":"agentMessage","text":"last reply"}]},
            {"id":"active","status":"inProgress","items":[{"id":"tool","type":"toolActivity"}]}],
            "desktop":{"status":"waiting","activeTurnId":"active","clarifications":[{"requestId":1}]}});
        compact(&mut thread);
        assert_eq!(thread["turns"][0]["items"].as_array().unwrap().len(),1);
        assert_eq!(thread["turns"][0]["items"][0]["id"],"a");
        assert_eq!(thread["turns"][1]["id"],"active");
        assert_eq!(thread["desktop"]["status"],"waiting");
        assert_eq!(thread["desktop"]["clarifications"][0]["requestId"],1);
        assert_eq!(thread["historyCursor"],"latest");
    }
}
