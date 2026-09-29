//! History probe. May open a dormant chat in the original desktop; never sends.
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
    #[cfg(target_os = "macos")]
    if std::env::args().nth(1).as_deref() == Some("--creation-preflight") {
        bridge.creation_preflight()?;
        println!("Creation UI preflight passed; no navigation or sending performed.");
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("--history") {
        let thread = std::env::args().nth(2).ok_or("Missing thread ID")?;
        let start = std::time::Instant::now();
        let result = bridge.dispatch("desktop/thread/history", &json!({"threadId":thread}))?;
        println!(
            "{}",
            json!({"elapsedMs":start.elapsed().as_millis(), "source":result["thread"]["desktop"]["source"], "items":result["thread"]["turns"].as_array().map(Vec::len),"nextCursor":result["nextCursor"]})
        );
        return Ok(());
    }
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
