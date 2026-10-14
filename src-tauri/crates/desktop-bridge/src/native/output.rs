//! Materialize images actually emitted by the engine; remote callers never supply paths.
use crate::attachments::Attachments;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{fs, io::Read, path::Path};
const MAX_IMAGE: usize = 5 * 1024 * 1024;

fn image(store: &Attachments, value: &str, raw: bool) -> Value {
    let encoded = if raw { Some(value) } else {
        value.strip_prefix("data:image/png;base64,").or_else(||value.strip_prefix("data:image/jpeg;base64,"))
    };
    let result = encoded.filter(|v|v.len()<=MAX_IMAGE*4/3+4)
        .and_then(|v| STANDARD.decode(v).ok()).and_then(|v|store.import_image(&v).ok());
    result.map(|id|json!({"type":"image","imageId":id}))
        .unwrap_or(json!({"type":"image","unavailable":true}))
}
fn local_image(store:&Attachments,path:&str)->Value {
    let path=Path::new(path);
    let bytes=(|| {
        if !path.is_absolute() {return None;}
        let meta=fs::symlink_metadata(path).ok()?;
        if !meta.is_file() || meta.len()>MAX_IMAGE as u64 {return None;}
        let mut bytes=Vec::new();fs::File::open(path).ok()?.take((MAX_IMAGE+1) as u64).read_to_end(&mut bytes).ok()?;
        Some(bytes)
    })();
    bytes.and_then(|v|store.import_image(&v).ok()).map(|id|json!({"type":"image","imageId":id}))
        .unwrap_or(json!({"type":"image","unavailable":true}))
}
fn parts(store:&Attachments,items:&Value)->Vec<Value> {
    items.as_array().into_iter().flatten().take(128).filter_map(|p|match p["type"].as_str() {
        Some("text"|"inputText")=>p["text"].as_str().map(|s|json!({"type":"text","text":s})),
        Some("image")=>Some(image(store,p["data"].as_str().unwrap_or(""),true)),
        Some("inputImage")=>Some(image(store,p["imageUrl"].as_str().unwrap_or(""),false)),
        Some("resource")=>p["resource"]["text"].as_str().map(|s|json!({"type":"text","text":s})),
        Some("resource_link")=>Some(json!({"type":"text","text":p["uri"].as_str().unwrap_or("")})),
        _=>None,
    }).collect()
}
pub(crate) fn project(store:&Attachments,item:&mut Value) {
    let kind=item["type"].as_str().unwrap_or("").to_owned();
    if kind=="commandExecution" {
        // Execution records are status/result cards on mobile. Pending approval
        // requests use a separate path and retain the exact command for consent.
        if let Some(fields)=item.as_object_mut() {
            fields.retain(|key,_|matches!(key.as_str(),"id"|"type"|"status"|"cwd"|"aggregatedOutput"|"exitCode"|"durationMs"|"truncated"));
            fields.insert("command".into(),json!("执行命令"));
        }
        return;
    }
    if matches!(kind.as_str(),"mcpToolCall"|"dynamicToolCall") {
        if let Some(fields)=item.as_object_mut() {fields.remove("arguments");}
    }
    let content=match kind.as_str() {
        "mcpToolCall"=>parts(store,&item["result"]["content"]),
        "dynamicToolCall"=>parts(store,&item["contentItems"]),
        "imageGeneration"=>vec![image(store,item["result"].as_str().unwrap_or(""),true)],
        "imageView"=>vec![local_image(store,item["path"].as_str().unwrap_or(""))],
        _=>return,
    };
    item["outputContent"]=json!(content);
    // Images are chunk-read from immutable account storage, never repeated in snapshots.
    if kind=="mcpToolCall" {
        if let Some(result)=item["result"].as_object_mut() {result.remove("content");result.remove("_meta");}
    }
    if kind=="dynamicToolCall" {item.as_object_mut().unwrap().remove("contentItems");}
    if kind=="imageGeneration" {item.as_object_mut().unwrap().remove("result");}
}

pub(crate) fn project_items(store:&Attachments,thread:&mut Value) {
    for item in thread["turns"].as_array_mut().into_iter().flatten()
        .flat_map(|turn|turn["items"].as_array_mut().into_iter().flatten()) {
        project(store,item);
        if item["type"]=="userMessage" {item["content"]=json!(store.project_content(&item["content"]));}
    }
}

pub(crate) fn project_batch(store:&Attachments,batch:&mut Value) {
    for entry in batch["events"].as_array_mut().into_iter().flatten() {
        let message=&mut entry["message"];
        match message["method"].as_str() {
            Some("item/started"|"item/completed")=>{
                let item=&mut message["params"]["item"];
                project(store,item);
                if item["type"]=="userMessage" {item["content"]=json!(store.project_content(&item["content"]));}
            },
            Some("turn/started"|"turn/completed")=>{
                for item in message["params"]["turn"]["items"].as_array_mut().into_iter().flatten() {
                    project(store,item);
                    if item["type"]=="userMessage" {item["content"]=json!(store.project_content(&item["content"]));}
                }
            },
            _=>{},
        }
    }
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn execution_scripts_are_not_mobile_output() {
        let root=tempfile::tempdir().unwrap();let store=Attachments::open(root.path()).unwrap();
        let mut command=json!({"id":"c","type":"commandExecution","status":"completed",
            "command":"PRIVATE_SCRIPT".repeat(10000),"commandActions":[{"command":"PRIVATE_SCRIPT"}],
            "cwd":"/repo","aggregatedOutput":"RESULT_OK","exitCode":0,"durationMs":12});
        let before=command.to_string().len();
        project(&store,&mut command);
        assert!(!command.to_string().contains("PRIVATE_SCRIPT"));
        assert_eq!(command["aggregatedOutput"],"RESULT_OK");assert_eq!(command["exitCode"],0);
        assert_eq!(command["status"],"completed");assert!(command.to_string().len()<256);
        println!("COMMAND_PAYLOAD_BYTES: {before} -> {}",command.to_string().len());
        let mut tool=json!({"id":"t","type":"dynamicToolCall","tool":"exec_command",
            "arguments":{"cmd":"PRIVATE_SCRIPT"},"contentItems":[{"type":"text","text":"RESULT_OK"}]});
        project(&store,&mut tool);assert!(tool.get("arguments").is_none());
        assert_eq!(tool["outputContent"][0]["text"],"RESULT_OK");
        let raw=json!({"id":"c","type":"commandExecution","command":"PRIVATE_SCRIPT","status":"inProgress"});
        let approval=json!({"id":9,"method":"item/commandExecution/requestApproval","params":{"command":"EXACT_APPROVAL","threadId":"t"}});
        let mut batch=json!({"pendingRequests":[approval.clone()],"events":[
            {"message":{"method":"item/started","params":{"item":raw.clone()}}},
            {"message":{"method":"turn/completed","params":{"turn":{"id":"u","items":[raw.clone()]}}}},
            {"message":approval.clone()}]});
        project_batch(&store,&mut batch);
        assert!(!batch.to_string().contains("PRIVATE_SCRIPT"));
        assert_eq!(batch["pendingRequests"][0],approval);assert_eq!(batch["events"][2]["message"],approval);
        let mut thread=json!({"turns":[{"items":[raw]}]});
        project_items(&store,&mut thread);assert!(!thread.to_string().contains("PRIVATE_SCRIPT"));
        let projected=crate::owner_output::project(&json!({"id":"c","type":"commandExecution","command":"PRIVATE_SCRIPT","exitCode":0}),&store,&mut 1000).unwrap();
        assert!(!projected.to_string().contains("PRIVATE_SCRIPT"));assert_eq!(projected["exitCode"],0);
    }
    #[test] fn image_outputs_use_private_content_ids_without_remote_paths() {
        let root=tempfile::tempdir().unwrap();let store=Attachments::open(root.path()).unwrap();
        let bytes=b"\x89PNG\r\n\x1a\nabcdefghijklmnop";
        let mut item=json!({"type":"mcpToolCall","result":{"content":[{"type":"image","mimeType":"image/png","data":STANDARD.encode(bytes)},{"type":"text","text":"screen"}],"_meta":{"private":"unused"}}});
        project(&store,&mut item);
        let id=item["outputContent"][0]["imageId"].as_str().unwrap();
        assert!(crate::attachments::valid_id(id));
        assert!(item["result"].get("content").is_none());
        let read=store.read(&json!({"imageId":id,"offset":0})).unwrap();
        assert_eq!(read["data"],STANDARD.encode(bytes));
        assert_eq!(item["outputContent"][1]["text"],"screen");
        let mut external=json!({"type":"dynamicToolCall","contentItems":[{"type":"inputImage","imageUrl":"https://private.invalid/screenshot"}]});
        project(&store,&mut external);assert_eq!(external["outputContent"][0]["unavailable"],true);
    }
}
