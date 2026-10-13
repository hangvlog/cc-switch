//! Read only the current account's local unread bucket. Writes go through official IPC.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::Path,
    time::{Duration, Instant},
};

fn document(path: &Path, max: u64) -> Option<Value> {
    let mut data = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(max + 1)
        .read_to_end(&mut data)
        .ok()?;
    if data.len() as u64 > max {
        return None;
    }
    serde_json::from_slice(&data).ok()
}
fn context(home: &Path) -> Option<(Value, String, String)> {
    let auth = document(&home.join("auth.json"), 128 * 1024)?;
    if auth["auth_mode"]
        .as_str()
        .is_some_and(|mode| mode != "chatgpt")
    {
        return None;
    }
    let token = auth["tokens"]["access_token"].as_str()?;
    let claims: Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(token.split('.').nth(1)?.trim_end_matches('='))
            .ok()?,
    )
    .ok()?;
    let claims = &claims["https://api.openai.com/auth"];
    let account = claims["chatgpt_account_id"]
        .as_str()
        .or_else(|| claims["account_id"].as_str())?;
    let user = claims["user_id"]
        .as_str()
        .or_else(|| claims["chatgpt_user_id"].as_str())?;
    let identity = json!({"kind":"chatgpt","accountId":account,"userId":user});
    let key = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&json!(["chatgpt", account, user])).ok()?)
    );
    let host = format!("local:{:x}", Sha256::digest(b"[\"local\",\"local\",null]"));
    Some((
        json!({"identity":identity,"executionHostKey":host}),
        key,
        host,
    ))
}
fn bucket(home: &Path) -> Option<(Value, Vec<String>)> {
    let (context, key, host) = context(home)?;
    let state = document(&home.join(".codex-global-state.json"), 8 * 1024 * 1024)?;
    let ids = state["electron-thread-read-state-v1"]["unreadByIdentity"][key][host].as_array()?;
    Some((
        context,
        ids.iter()
            .filter_map(|id| id.as_str().map(str::to_owned))
            .collect(),
    ))
}
pub fn annotate(home: &Path, threads: &mut [Value]) {
    let Some((_, ids)) = bucket(home) else { return };
    for thread in threads {
        thread["hasUnreadTurn"] = json!(ids.iter().any(|id| thread["id"] == *id));
    }
}
pub fn mark_read(home: &Path, thread: &str) -> Result<Value, String> {
    let (before, ids) = bucket(home).ok_or("当前账号未读状态尚未就绪")?;
    if !ids.iter().any(|id| id == thread) {
        return Ok(json!({"read":true}));
    }
    let mut client = crate::ipc::IpcClient::connect(home)?;
    client.broadcast_read_state(thread, before.clone())?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Some((current, ids)) = bucket(home) {
            if current != before {
                return Err("账号已变化，请重新打开会话".into());
            }
            if !ids.iter().any(|id| id == thread) {
                return Ok(json!({"read":true}));
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("电脑未确认已读状态；请在任务结束后正常退出 Codex，再从 ClawKit 打开".into())
}

#[cfg(all(test, windows))]
mod windows_live_tests {
    #[test]
    #[ignore = "Touches only a unique synthetic unread ID in the running official desktop"]
    fn official_desktop_acknowledges_synthetic_read_state() {
        let home=std::path::PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(".codex");
        let (context,_,_)=super::context(&home).expect("ChatGPT authentication required");
        let thread=uuid::Uuid::new_v4().to_string();
        let mut client=crate::ipc::IpcClient::connect(&home).unwrap();
        struct Cleanup<'a> {client:&'a mut crate::ipc::IpcClient,thread:String,context:serde_json::Value}
        impl Drop for Cleanup<'_> {fn drop(&mut self){let _=self.client.broadcast_read_state(&self.thread,self.context.clone());}}
        let cleanup=Cleanup{client:&mut client,thread:thread.clone(),context:context.clone()};
        cleanup.client.broadcast_test_unread(&thread,context).unwrap();
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(5);
        while std::time::Instant::now()<deadline {
            if super::bucket(&home).is_some_and(|(_,ids)|ids.contains(&thread)){break;}
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(super::bucket(&home).is_some_and(|(_,ids)|ids.contains(&thread)),"Official desktop did not persist the synthetic unread broadcast");
        assert_eq!(super::mark_read(&home,&thread).unwrap()["read"],true);
        assert!(!super::bucket(&home).unwrap().1.contains(&thread));
    }
}
