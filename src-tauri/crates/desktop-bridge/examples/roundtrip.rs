//! Explicit isolated desktop E2E: create the synthetic task in the test app first.
use clawkit_desktop_bridge::Bridge;
use serde_json::json;
use std::path::PathBuf;

fn main() -> Result<(), String> {
    let home = PathBuf::from(
        std::env::var("CLAWKIT_TEST_HOME").map_err(|_| "CLAWKIT_TEST_HOME is required")?,
    );
    if !home.is_absolute() || !home.to_string_lossy().contains("clawkit-owner-e2e-") {
        return Err("Only an isolated test home is allowed".into());
    }
    let thread = std::env::args()
        .nth(1)
        .ok_or("test thread id is required")?;
    let storage = home
        .parent()
        .ok_or("invalid test home")?
        .join("dispatch-e2e");
    let bridge = Bridge::open(&home, &storage)?;
    let before = bridge.dispatch("desktop/thread/read", &json!({"threadId":thread}))?;
    if !before["thread"]["preview"]
        .as_str()
        .unwrap_or("")
        .contains("隔离测试")
    {
        return Err("Expected synthetic test task title".into());
    }
    if before["thread"]["desktop"]["canSend"] != true {
        return Err(before["thread"]["desktop"].to_string());
    }
    let id = uuid::Uuid::new_v4().to_string();
    let prompt = format!(
        "ClawKit 隔离桥接验证 {}。请只回复 OWNER_BRIDGE_OK，不使用工具。",
        id
    );
    let params = json!({"threadId":thread,"requestId":id,"text":prompt});
    let receipt = bridge.dispatch("desktop/turn/start", &params)?;
    println!("receipt={receipt}");
    if receipt["status"] != "accepted" {
        return Err("Unknown receipt: inspect desktop, never resend".into());
    }
    drop(bridge);
    let bridge = Bridge::open(&home, &storage)?;
    assert_eq!(
        bridge.dispatch("desktop/turn/start", &params)?["status"],
        "accepted"
    );
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let read = bridge.dispatch("desktop/thread/read", &json!({"threadId":thread}))?;
        let turns = read["thread"]["turns"].as_array().ok_or("missing turns")?;
        let matching_users = turns
            .iter()
            .flat_map(|turn| turn["items"].as_array().into_iter().flatten())
            .filter(|item| item["type"] == "userMessage" && item["content"][0]["text"] == prompt)
            .count();
        if read["thread"]["desktop"]["status"] == "idle" {
            let answer = turns
                .last()
                .into_iter()
                .flat_map(|turn| turn["items"].as_array().into_iter().flatten())
                .any(|item| {
                    item["type"] == "agentMessage"
                        && item["text"]
                            .as_str()
                            .is_some_and(|text| text.trim() == "OWNER_BRIDGE_OK")
                });
            assert_eq!(matching_users, 1, "Duplicate send after ledger reopen");
            assert!(answer, "Expected assistant answer");
            assert_eq!(
                read["thread"]["desktop"]["model"],
                before["thread"]["desktop"]["model"]
            );
            println!("OWNER_ROUNDTRIP_OK duplicate_count=1 settings=inherited");
            return Ok(());
        }
    }
    Err("Model did not complete within the bounded E2E observation window".into())
}
