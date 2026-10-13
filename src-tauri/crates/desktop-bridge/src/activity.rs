//! Offline preview fallback only. Runtime status must come from the original owner.
use serde_json::{json, Value};
use std::{fs::File, io::{BufRead, BufReader, Read, Seek, SeekFrom}, path::Path};

pub fn read(home: &Path, thread: &str, raw_path: &str) -> Option<Value> {
    let home=home.canonicalize().ok()?;
    let path=Path::new(raw_path).canonicalize().ok()?;
    if !path.starts_with(home.join("sessions")){return None;}
    let mut file=File::open(path).ok()?;
    let mut header=String::new();
    BufReader::new((&mut file).take(1024*1024)).read_line(&mut header).ok()?;
    let meta:Value=serde_json::from_str(&header).ok()?;
    if meta["type"]!="session_meta" || meta["payload"]["id"]!=thread{return None;}
    let size=file.metadata().ok()?.len();let start=size.saturating_sub(512*1024);
    file.seek(SeekFrom::Start(start)).ok()?;let mut bytes=Vec::new();file.take(512*1024).read_to_end(&mut bytes).ok()?;
    let mut summary=None;
    for (index,line) in bytes.split(|b|*b==b'\n').enumerate(){
        if start>0 && index==0 {continue;}
        let Ok(event)=serde_json::from_slice::<Value>(line) else {continue;};
        let p=&event["payload"];
        if event["type"]=="event_msg" {
            match p["type"].as_str(){
                Some("agent_message")=> {if let Some(text)=p["message"].as_str(){summary=Some(text.to_owned());}},
                _=>{}
            }
        } else if event["type"]=="response_item" && p["type"]=="message" && p["role"]=="assistant" && matches!(p["phase"].as_str(),Some("commentary"|"final")) {
            let text=p["content"].as_array().into_iter().flatten().filter_map(|c|c["text"].as_str()).collect::<Vec<_>>().join(" ");
            if !text.is_empty(){summary=Some(text);}
        }
    }
    Some(json!({"summary":summary.map(|s|preview(&s))}))
}

/// The desktop reads the newest visible agentMessage, including streamed commentary.
pub fn live_summary(state: &Value) -> Option<String> {
    for turn in crate::history::turns(state).into_iter().rev() {
        for item in turn["items"].as_array().into_iter().flatten().rev() {
            if item["type"] != "agentMessage" || item["phase"] == "analysis" || item["channel"] == "analysis" { continue; }
            if let Some(text) = item["text"].as_str().filter(|s| !s.trim().is_empty()) { return Some(preview(text)); }
        }
    }
    None
}
fn preview(text: &str) -> String {
    text.trim().chars().take(1200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reply_comes_from_visible_assistant_not_tools_or_analysis() {
        let home=tempfile::tempdir().unwrap();std::fs::create_dir(home.path().join("sessions")).unwrap();
        let path=home.path().join("sessions/t.jsonl");
        let events=[json!({"type":"session_meta","payload":{"id":"t"}}),
            json!({"type":"event_msg","payload":{"type":"task_started"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","phase":"analysis","content":[{"text":"private reasoning"}]}}),
            json!({"type":"event_msg","payload":{"type":"agent_message","message":"visible reply"}}),
            json!({"type":"event_msg","payload":{"type":"task_complete"}})];
        std::fs::write(&path,events.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")).unwrap();
        let result=read(home.path(),"t",path.to_str().unwrap()).unwrap();
        assert_eq!(result["summary"],"visible reply");assert!(result.get("status").is_none());
        assert!(read(home.path(),"other",path.to_str().unwrap()).is_none());
    }
}
