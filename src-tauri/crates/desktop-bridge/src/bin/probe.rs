//! History probe. May open a dormant chat in the original desktop; never sends.
use clawkit_desktop_bridge::Bridge;
use serde_json::json;
use std::path::PathBuf;
fn main() -> Result<(), String> {
    clawkit_desktop_bridge::maybe_run_router();
    clawkit_desktop_bridge::native::maybe_run_adapter();
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".codex")
        });
    #[cfg(target_os = "macos")]
    if let Some(flag @ ("--ordinary-launch-enable" | "--ordinary-launch-disable")) = std::env::args().nth(1).as_deref() {
        clawkit_desktop_bridge::native::ordinary_launch::configure(
            &std::env::current_exe().map_err(|e| e.to_string())?, flag.ends_with("enable"))?;
        return Ok(());
    }
    #[cfg(unix)]
    if std::env::args().nth(1).as_deref() == Some("--maintain-router") {
        let exe=std::env::current_exe().map_err(|e|e.to_string())?;
        let stop=std::env::args().nth(2).ok_or("Missing stop file")?;
        while !std::path::Path::new(&stop).exists() {
            clawkit_desktop_bridge::maintain_background_router(&home,&exe)?;
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        return Ok(());
    }
    let storage = std::env::var_os("CLAWKIT_PROBE_STORAGE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("clawkit-bridge-probe-{}", std::process::id()))
        });
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
    if std::env::args().nth(1).as_deref() == Some("--owner-stream") {
        let thread=std::env::args().nth(2).ok_or("Missing thread")?;
        let watch=uuid::Uuid::new_v4().to_string();
        bridge.dispatch("desktop/owner/watch",&json!({"threadId":thread,"watchId":watch}))?;
        let streams=bridge.owner_streams();let until=std::time::Instant::now()+std::time::Duration::from_secs(40);
        while std::time::Instant::now()<until {
            for event in streams.drain() {let t=&event["params"]["thread"];
                println!("{}",json!({"sequence":event["params"]["sequence"],"revision":t["desktop"]["revision"],"source":t["desktop"]["source"],"status":t["desktop"]["status"],"bytes":event.to_string().len()}));
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        bridge.dispatch("desktop/owner/unwatch",&json!({"threadId":thread,"watchId":watch}))?;return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("--models") {
        println!("{}", bridge.dispatch("desktop/model/list", &json!({}))?);
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("--drain") {
        bridge.drain_queue()?;
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("--request") {
        use std::io::Read;
        let mut input = String::new();
        std::io::stdin()
            .read_to_string(&mut input)
            .map_err(|e| e.to_string())?;
        let request: serde_json::Value = serde_json::from_str(&input).map_err(|e| e.to_string())?;
        println!("{}", bridge.handle(request).ok_or("Request ID required")?);
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
