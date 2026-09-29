//! Explicit synthetic original-window creation acceptance. Reuse the request ID to query only.
use clawkit_desktop_bridge::Bridge;
use serde_json::json;
use std::path::PathBuf;
fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 5 || args[1] != "--send-synthetic" {
        return Err("Usage: create_project --send-synthetic <project UUID> <request UUID> <receipt directory>".into());
    }
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME missing")?).join(".codex");
    let bridge = Bridge::open(&home, &PathBuf::from(&args[4]))?;
    let params = json!({"projectId":args[2],"requestId":args[3],"text":"Reply exactly CLAWKIT_PROJECT_CREATE_929. Do not use tools or change files."});
    let receipt = bridge.dispatch("desktop/thread/create", &params)?;
    println!("{receipt}");
    if receipt["status"] != "accepted" {
        return Err("Inspect original desktop, do not resend".into());
    }
    let duplicate = bridge.dispatch("desktop/thread/create", &params)?;
    assert_eq!(receipt, duplicate);
    println!("CREATION_RECEIPT_DEDUP_OK");
    Ok(())
}
