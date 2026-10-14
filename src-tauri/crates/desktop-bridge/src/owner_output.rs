//! Public tool fields from the original owner. Never forward its full internal item.
use crate::attachments::Attachments;
use serde_json::{json, Map, Value};

fn bounded(value: &Value, budget: &mut usize, depth: usize) -> Value {
    if *budget == 0 || depth > 12 { return Value::Null; }
    *budget -= 1;
    match value {
        Value::String(s) => {
            let text: String = s.chars().take((*budget).min(24_000)).collect();
            *budget = budget.saturating_sub(text.chars().count());
            Value::String(text)
        }
        Value::Array(items) => Value::Array(items.iter().take(128)
            .map(|v| bounded(v, budget, depth + 1)).collect()),
        Value::Object(fields) => Value::Object(fields.iter().take(128)
            .filter(|(k, _)| !matches!(k.as_str(), "_meta" | "encryptedContent" | "encrypted_content"))
            .map(|(k, v)| (k.chars().take(200).collect(), bounded(v, budget, depth + 1))).collect()),
        _ => value.clone(),
    }
}

pub fn project(item: &Value, store: &Attachments, budget: &mut usize) -> Option<Value> {
    if *budget == 0 { return None; }
    let kind = item["type"].as_str()?;
    let fields: &[&str] = match kind {
        "commandExecution" | "fileChange" | "webSearch" => &[],
        "mcpToolCall" => &["result"],
        "dynamicToolCall" => &["contentItems"],
        "imageGeneration" => &["result"],
        "imageView" => &["path"],
        _ => return None,
    };
    let id = item["id"].as_str().filter(|s| !s.is_empty())?;
    let mut output = Map::new();
    output.insert("type".into(), json!(kind));
    output.insert("id".into(), json!(id));
    for field in ["status"].iter().chain(fields.iter()) {
        if let Some(value) = item.get(*field) { output.insert((*field).into(), value.clone()); }
    }
    let mut value = Value::Object(output);
    // Materialize only images emitted in owner state into private immutable storage.
    crate::native::output::project(store, &mut value);
    let before = *budget;
    let mut output = bounded(&value, budget, 0);
    // Keep routing identity intact even when the display budget is exhausted.
    output["id"] = json!(id); output["type"] = value["type"].clone();
    if let Some(status) = item.get("status") { output["status"] = status.clone(); }
    if *budget == 0 && before > 0 { output["truncated"] = json!(true); }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_projection_bounds_text_and_keeps_images_private() {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let root = tempfile::tempdir().unwrap(); let store = Attachments::open(root.path()).unwrap();
        let item = json!({"type":"mcpToolCall","id":"tool","tool":"render","private":"omit",
            "result":{"content":[{"type":"image","data":STANDARD.encode(b"\x89PNG\r\n\x1a\nabcdefghijklmnop")}],
                "structuredContent":{"saved":true},"_meta":{"hidden":true}}});
        let result = project(&item,&store,&mut 10000).unwrap();
        assert!(crate::attachments::valid_id(result["outputContent"][0]["imageId"].as_str().unwrap()));
        assert!(result.get("result").is_none());assert!(result.get("tool").is_none());
        assert!(result["result"].get("_meta").is_none());
        assert!(result.get("private").is_none());
        let command = json!({"id":"cmd","type":"commandExecution","status":"completed","aggregatedOutput":"字".repeat(500)});
        let mut budget = 80; let limited = project(&command,&store,&mut budget).unwrap();
        assert!(budget>0); assert!(limited.get("truncated").is_none());
        assert_eq!(limited["status"],"completed");
        assert!(limited.get("aggregatedOutput").is_none());assert_eq!(limited["type"],"toolActivity");
        assert!(project(&json!({"id":"r","type":"reasoning","rawContent":"hidden"}),&store,&mut 100).is_none());
    }
}
