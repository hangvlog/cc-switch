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

#[cfg(test)] mod tests {
    use super::*;
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
