//! Restricted access to the original desktop's initialized app-server connection.
//! The desktop retains all notifications, tool requests and approvals.
pub(crate) mod creation;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
#[cfg(unix)]
mod runtime;
#[cfg(unix)]
mod transport;
#[cfg(unix)]
pub use runtime::maybe_run_adapter;
#[cfg(not(unix))]
pub fn maybe_run_adapter() {}

#[cfg(target_os = "macos")]
mod launch;
#[cfg(target_os = "macos")]
pub use launch::launch;

pub(crate) fn socket_path(home: &Path) -> PathBuf {
    home.join("clawkit-runtime").join("engine.sock")
}

pub fn ready(home: &Path) -> bool {
    request(home, "clawkit/status", json!({}))
        .is_ok_and(|value| value["ready"] == true && value["protocolVersion"] == 1)
}

pub(crate) fn request(home: &Path, method: &str, params: Value) -> Result<Value, String> {
    #[cfg(unix)]
    {
        transport::request(&socket_path(home), method, params)
    }
    #[cfg(not(unix))]
    {
        let _ = (home, method, params);
        Err("此系统尚未支持原生新建通道".into())
    }
}

// The local endpoint is not a generic app-server proxy, even for an authenticated relay.
fn allowed(method: &str, params: &Value) -> bool {
    let Some(object) = params.as_object() else {
        return false;
    };
    let keys: &[&str] = match method {
        "clawkit/status" => &[],
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
