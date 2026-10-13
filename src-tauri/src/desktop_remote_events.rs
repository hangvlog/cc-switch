//! Event pump is independent of RPC dispatch and never holds the bridge mutex.
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};
use tokio::sync::mpsc;

pub async fn pump(owner: clawkit_desktop_bridge::owner_stream::OwnerStreams, home: PathBuf, storage: PathBuf, output: mpsc::Sender<Value>) {
    let mut cursor = json!({});
    loop {
        for event in owner.drain() { if output.send(event).await.is_err() {return;} }
        let home = home.clone();
        let params = cursor.clone();
        let storage = storage.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            let mut batch=clawkit_desktop_bridge::native::read_events(&home, params)?;
            clawkit_desktop_bridge::native::project_event_outputs(&mut batch,&storage)?;
            Ok::<_,String>(batch)
        }).await;
        match result {
            Ok(Ok(batch)) => {
                cursor = json!({"engineInstanceId":batch["engineInstanceId"],"afterSeq":batch["cursor"]});
                if batch["resetRequired"] == true || batch["events"].as_array().is_some_and(|v|!v.is_empty()) {
                    if output.send(json!({"method":"desktop/events","params":batch})).await.is_err() {return;}
                }
            }
            _ => { cursor = json!({}); }
        }
        if output.is_closed() {return;}
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// Chunk large snapshots below the relay's frame limit without cutting UTF-8 codepoints.
pub fn frames(value: Value) -> Vec<String> {
    let raw=value.to_string();
    if raw.len()<=64*1024 {return vec![json!({"type":"relay.data","payload":raw}).to_string()];}
    let id=uuid::Uuid::new_v4().to_string();
    let mut chunks=Vec::new(); let mut start=0;
    while start<raw.len() {
        let mut end=(start+64*1024).min(raw.len());
        while !raw.is_char_boundary(end) {end-=1;}
        chunks.push(&raw[start..end]); start=end;
    }
    let total=chunks.len();
    chunks.into_iter().enumerate().map(|(index,chunk)|json!({"type":"relay.data","payload":
        json!({"method":"desktop/segment","params":{"id":id,"index":index,"total":total,"chunk":chunk}}).to_string()}).to_string()).collect()
}
