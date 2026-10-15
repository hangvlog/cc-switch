//! Keep current runtime/interaction state while deferring older visible messages.
use serde_json::{json, Value};

pub fn compact(thread: &mut Value) {
    let Some(turns) = thread["turns"].as_array() else { return };
    let mut keep = Vec::new();
    let mut reply = false;
    'outer: for (t, turn) in turns.iter().enumerate().rev() {
        let Some(items) = turn["items"].as_array() else { continue };
        for (i, item) in items.iter().enumerate().rev() {
            if item["phase"] == "analysis" || item["channel"] == "analysis" { continue; }
            if item["type"] == "userMessage" { keep.push((t, i)); break 'outer; }
            if item["type"] == "agentMessage" && !reply { keep.push((t, i)); reply = true; }
        }
    }
    let output: Vec<_> = turns.iter().enumerate().filter_map(|(index, turn)| {
        let mut result = turn.clone();
        let items: Vec<_> = turn["items"].as_array().into_iter().flatten().enumerate()
            .filter(|(i, _)| keep.contains(&(index, *i))).map(|(_, item)| item).collect();
        if items.is_empty() && turn["status"] != "inProgress" { return None; }
        result["items"] = json!(items);
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
        assert_eq!(thread["turns"][0]["items"].as_array().unwrap().len(),2);
        assert_eq!(thread["turns"][0]["items"][0]["id"],"u");
        assert_eq!(thread["turns"][1]["id"],"active");
        assert_eq!(thread["desktop"]["status"],"waiting");
        assert_eq!(thread["desktop"]["clarifications"][0]["requestId"],1);
        assert_eq!(thread["historyCursor"],"latest");
    }
}
