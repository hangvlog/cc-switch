//! Restricted access to the original desktop's initialized app-server connection.
//! The desktop retains its connection; human replies are arbitrated by the adapter.
pub(crate) mod creation;
mod control;
pub(crate) mod output;
mod events;
mod reminders;
pub(crate) mod responses;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
mod runtime;
mod transport;
#[cfg(unix)]
#[path="endpoint_unix.rs"]
mod endpoint;
#[cfg(windows)]
#[path="endpoint_windows.rs"]
pub(crate) mod endpoint;
pub use runtime::maybe_run_adapter;

#[cfg(target_os = "macos")]
mod launch;
#[cfg(windows)]
#[path="launch_windows.rs"]
mod launch;
#[cfg(any(target_os = "macos",windows))]
pub use launch::launch;

pub(crate) fn socket_path(home: &Path) -> PathBuf {
    #[cfg(unix)] {home.join("clawkit-runtime").join("engine.sock")}
    #[cfg(windows)] {endpoint::socket_path(home)}
}

pub fn read_events(home: &Path, params: Value) -> Result<Value, String> {
    request(home, "clawkit/events/read", params)
}

pub fn project_event_outputs(batch:&mut Value,storage:&Path)->Result<(),String> {
    let attachments=crate::attachments::Attachments::open(storage)?;
    output::project_batch(&attachments,batch);
    Ok(())
}

pub fn event_ready(home: &Path) -> bool {
    request(home, "clawkit/status", json!({})).is_ok_and(|v|v["ready"]==true && v["eventProtocolVersion"]==1)
}

pub fn ready(home: &Path) -> bool {
    request(home, "clawkit/status", json!({}))
        .is_ok_and(|value| value["ready"] == true && value["protocolVersion"] == 1)
}

pub(crate) fn request(home: &Path, method: &str, params: Value) -> Result<Value, String> {
    transport::request(&socket_path(home), method, params)
}

// The local endpoint is not a generic app-server proxy, even for an authenticated relay.
fn allowed(method: &str, params: &Value) -> bool {
    let Some(object) = params.as_object() else {
        return false;
    };
    let keys: &[&str] = match method {
        "clawkit/status" | "clawkit/notifications/read" => &[],
        "clawkit/events/read" => &["engineInstanceId", "afterSeq"],
        "clawkit/request/respond" => &["engineInstanceId", "threadId", "serverRequestId", "decision", "answers", "content"],
        "turn/interrupt" => &["threadId", "turnId"],
        "thread/resume" => &["threadId", "excludeTurns", "initialTurnsPage"],
        "turn/steer" => &["threadId", "expectedTurnId", "clientUserMessageId", "input"],
        "thread/turns/list" => &["threadId", "cursor", "limit", "itemsView", "sortDirection"],
        "thread/settings/update" => &["threadId", "model"],
        "model/list" => &["includeHidden", "cursor", "limit"],
        "project/list" => &["cursor", "limit"],
        "project/read" => &["projectId"],
        "thread/start" => &["projectId", "cwd"],
        "turn/start" => &["threadId", "clientUserMessageId", "input"],
        "thread/read" => &["threadId", "includeTurns"],
        _ => return false,
    };
    object.keys().all(|key| keys.contains(&key.as_str()))
}

#[cfg(all(test, unix))]
#[path = "tests.rs"]
mod native_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_endpoint_rejects_arbitrary_methods_and_permission_overrides() {
        assert!(allowed(
            "thread/start",
            &json!({"projectId":"p","cwd":"/tmp"})
        ));
        assert!(!allowed("thread/start", &json!({"approvalPolicy":"never"})));
        assert!(!allowed("process/spawn", &json!({})));
        assert!(!allowed("account/read", &json!({})));
        assert!(!allowed(
            "item/commandExecution/requestApproval",
            &json!({})
        ));
    }
}
