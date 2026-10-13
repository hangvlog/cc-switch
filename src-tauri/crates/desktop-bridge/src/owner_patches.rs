//! Apply the official Immer JSON patch subset. A failed batch invalidates the
//! local copy; callers must obtain a fresh owner snapshot before publishing.
use serde_json::Value;

pub fn apply(state: &mut Value, patches: &Value) -> Result<(), String> {
    let patches = patches.as_array().filter(|p| p.len() <= 100_000).ok_or("Invalid owner patches")?;
    for patch in patches {
        let path = patch["path"].as_array().filter(|p| p.len() <= 128).ok_or("Invalid owner path")?;
        let op = patch["op"].as_str().ok_or("Missing patch operation")?;
        if !matches!(op, "add" | "replace" | "remove") { return Err("Unknown owner patch operation".into()); }
        if path.is_empty() {
            if op == "remove" { return Err("Cannot remove owner state".into()); }
            *state = patch.get("value").ok_or("Missing patch value")?.clone();
            continue;
        }
        let mut parent = &mut *state;
        for segment in &path[..path.len()-1] {
            parent = if let Some(key) = segment.as_str() { parent.get_mut(key) }
                else { segment.as_u64().and_then(|i| usize::try_from(i).ok()).and_then(|i| parent.get_mut(i)) }
                .ok_or("Owner patch parent missing")?;
        }
        let key = path.last().unwrap();
        match parent {
            Value::Object(fields) => {
                let key = key.as_str().ok_or("Invalid object key")?;
                if op != "add" && !fields.contains_key(key) { return Err("Owner patch field missing".into()); }
                if op == "remove" { fields.remove(key); }
                else { fields.insert(key.into(), patch.get("value").ok_or("Missing patch value")?.clone()); }
            }
            Value::Array(items) => {
                let index = key.as_u64().and_then(|i| usize::try_from(i).ok()).ok_or("Invalid array index")?;
                match op {
                    "add" if index <= items.len() => items.insert(index, patch.get("value").ok_or("Missing patch value")?.clone()),
                    "replace" if index < items.len() => items[index] = patch.get("value").ok_or("Missing patch value")?.clone(),
                    "remove" if index < items.len() => { items.remove(index); },
                    _ => return Err("Owner patch index out of range".into()),
                }
            }
            _ => return Err("Owner patch parent is scalar".into()),
        }
    }
    Ok(())
}

#[cfg(test)] mod tests {
    use super::*;
    use serde_json::json;
    #[test] fn official_paths_preserve_order_and_reject_gaps() {
        let mut s=json!({"turnHistory":{"history":{"entitiesByKey":{"tail:0:local:test":{"items":[{"text":"a"}]}}}}});
        let prefix=json!(["turnHistory","history","entitiesByKey","tail:0:local:test","items"]);
        let patch=|op:&str,i:u64,value:Value|{let mut path=prefix.as_array().unwrap().clone();path.push(json!(i));json!({"op":op,"path":path,"value":value})};
        apply(&mut s,&json!([patch("add",1,json!({"text":"b"})),patch("replace",0,json!({"text":"A"})),patch("remove",1,Value::Null)])).unwrap();
        assert_eq!(s["turnHistory"]["history"]["entitiesByKey"]["tail:0:local:test"]["items"],json!([{"text":"A"}]));
        assert!(apply(&mut s,&json!([patch("add",5,Value::Null)])).is_err());
        assert!(apply(&mut s,&json!([{"op":"move","path":[]}])).is_err());
    }
}
