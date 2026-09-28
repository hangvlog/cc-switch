//! Read-only probe. Intentionally exposes no send command.
use clawkit_desktop_bridge::Bridge;
use serde_json::json;
use std::path::PathBuf;
fn main() -> Result<(), String> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".codex")
        });
    let storage = std::env::temp_dir().join(format!("clawkit-bridge-probe-{}", std::process::id()));
    let bridge = Bridge::open(&home, &storage)?;
    let status = bridge.dispatch("desktop/status", &json!({}))?;
    println!("{}", status);
    if let Some(thread) = std::env::args().nth(1) {
        let result = bridge.dispatch("desktop/thread/read", &json!({"threadId":thread}))?;
        println!(
            "{}",
            json!({"desktop":result["thread"]["desktop"],"turnCount":result["thread"]["turns"].as_array().map(Vec::len)})
        );
    } else {
        let result = bridge.dispatch("desktop/thread/list", &json!({}))?;
        println!(
            "{}",
            json!({"taskCount":result["data"].as_array().map(Vec::len),"hasMore":!result["nextCursor"].is_null()})
        );
    }
    Ok(())
}
