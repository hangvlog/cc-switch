//! Explicit Claude desktop acceptance on this computer's real sessions.
//! Usage: claude_probe <storage dir> <method> '<json params>'
//! Desktop inbox deliveries return their receipt. CLI turns wait for the
//! `completed` event. Approvals follow
//! CLAUDE_PROBE_DECISION (accept | decline, default decline).
use clawkit_desktop_bridge::Bridge;
use serde_json::{json, Value};
use std::{path::PathBuf, time::{Duration, Instant}};

fn main() -> Result<(), String> {
    #[cfg(unix)]
    clawkit_desktop_bridge::claude::maybe_run_hook();
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        return Err("Usage: claude_probe <storage dir> <method> '<json params>'".into());
    }
    let storage = std::path::absolute(PathBuf::from(&args[1])).map_err(|e| e.to_string())?;
    let bridge = Bridge::open(&storage, &storage)?;
    let params: Value = serde_json::from_str(&args[3]).map_err(|e| e.to_string())?;
    let started = Instant::now();
    let result = bridge.dispatch(&args[2], &params)?;
    println!("{}", json!({"method":args[2],"ms":started.elapsed().as_millis(),"bytes":result.to_string().len(),"result":result}));
    if result["deliveredTo"] == "desktop" || !matches!(args[2].as_str(), "claude/turn/start" | "claude/thread/create") {
        return Ok(());
    }
    let decision = std::env::var("CLAUDE_PROBE_DECISION").unwrap_or_else(|_| "decline".into());
    let events = bridge.claude_events();
    let deadline = Instant::now() + Duration::from_secs(600);
    while Instant::now() < deadline {
        for event in events.drain() {
            println!("{event}");
            let p = &event["params"];
            if p["kind"] == "approval" {
                let reply = json!({"threadId":p["threadId"],"requestId":p["request"]["requestId"],"decision":decision});
                println!("{}", json!({"respond":bridge.dispatch("claude/request/respond", &reply)?}));
            }
            if p["kind"] == "completed" {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err("timed out".into())
}
