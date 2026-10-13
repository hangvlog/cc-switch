//! Conditional responses hash the visible projection, including edits to an
//! existing message. Timestamps of observation are transport metadata, not data.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub fn response(mut result: Value, known: Option<&str>) -> Value {
    let mut content = result["thread"].clone();
    if let Some(desktop) = content.get_mut("desktop").and_then(Value::as_object_mut) {
        desktop.remove("observedAt");
        desktop.remove("revision");
    }
    let version = format!("{:x}", Sha256::digest(content.to_string().as_bytes()));
    if known == Some(version.as_str()) {
        return json!({"notModified":true,"version":version,"desktop":result["thread"]["desktop"]});
    }
    result["version"] = json!(version);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unchanged_messages_transfer_no_content_but_edits_and_status_changes_invalidate() {
        let data = json!({"thread":{"id":"t","turns":[{"items":[{"id":"same-id","text":"partial"}]}],"desktop":{"status":"active","observedAt":1,"revision":1}}});
        let first = response(data.clone(), None);
        let mut next = data.clone();
        next["thread"]["desktop"]["observedAt"] = json!(2);
        next["thread"]["desktop"]["revision"] = json!(8);
        let unchanged = response(next.clone(), first["version"].as_str());
        assert_eq!(unchanged["notModified"], true);
        assert!(unchanged.get("thread").is_none());
        assert_eq!(unchanged["desktop"]["observedAt"], 2);
        next["thread"]["turns"][0]["items"][0]["text"] = json!("complete");
        assert!(response(next, first["version"].as_str()).get("thread").is_some());
        let mut idle = data;
        idle["thread"]["desktop"]["status"] = json!("idle");
        assert!(response(idle, first["version"].as_str()).get("thread").is_some());
    }
}
