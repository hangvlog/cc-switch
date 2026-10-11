use serde_json::{json, Value};
use std::path::Path;

// Build numbers are diagnostic only. Transport, snapshot and operation schemas
// are checked independently so unrelated desktop updates do not disable access.

#[cfg(target_os = "macos")]
pub(crate) fn running_pid(home: &Path) -> Option<i32> {
    use std::process::Command;
    let output = Command::new("/usr/sbin/lsof")
        .args(["-a", "-U", "-Fp"])
        .arg(home.join("ipc/ipc.sock"))
        .output()
        .ok()?;
    let pids = String::from_utf8(output.stdout).ok()?;
    let pid = pids.lines().find_map(|line| line.strip_prefix('p'))?;
    if !pid.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    pid.parse().ok()
}

#[cfg(target_os = "macos")]
pub(crate) fn running_contents(home: &Path) -> Option<std::path::PathBuf> {
    use std::process::Command;
    let pid = running_pid(home)?;
    let output = Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    let executable = String::from_utf8(output.stdout).ok()?;
    let contents = Path::new(executable.trim()).parent()?.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    Some(contents.to_path_buf())
}

#[cfg(target_os = "macos")]
fn running_build(home: &Path) -> Option<String> {
    use std::process::Command;
    let contents = running_contents(home)?;
    let read = |key: &str| -> Option<String> {
        let value = Command::new("/usr/bin/plutil")
            .args(["-extract", key, "raw", "-o", "-"])
            .arg(contents.join("Info.plist"))
            .output()
            .ok()?;
        if !value.status.success() {
            return None;
        }
        Some(String::from_utf8(value.stdout).ok()?.trim().into())
    };
    Some(format!(
        "{}:{}",
        read("CFBundleShortVersionString")?,
        read("CFBundleVersion")?
    ))
}

#[cfg(not(target_os = "macos"))]
fn running_build(_home: &Path) -> Option<String> {
    None
}

pub fn can_send(home: &Path) -> bool {
    crate::ipc::IpcClient::connect(home).is_ok()
}

pub fn status(home: &Path) -> Value {
    let build = running_build(home);
    json!({"mode":"desktop-owner","build":build,"compatibility":"operation-protocol",
        "canSend":can_send(home),"codexHome":home,
        "socketAvailable":home.join("ipc/ipc.sock").exists(),"nativeCreation":crate::native::ready(home)})
}

/// Ask the installed original desktop to load the conversation through its own
/// deep link. Never launch an app-server, claim ownership, or change task settings.
pub fn open_thread(home: &Path, thread: &str) -> Result<(), String> {
    if uuid::Uuid::parse_str(thread).is_err() {
        return Err("任务 ID 无效".into());
    }
    #[cfg(target_os = "macos")]
    {
        if !can_send(home) {
            return Err("Codex 桌面连接尚未就绪，请打开电脑上的 Codex 后重试".into());
        }
        let contents = running_contents(home).ok_or("找不到正在运行的 Codex 桌面")?;
        let app = contents.parent().ok_or("Codex 应用路径无效")?;
        let status = std::process::Command::new("/usr/bin/open")
            .args(["-g", "-a"])
            .arg(app)
            .arg(format!("codex://threads/{thread}"))
            .status()
            .map_err(|_| "无法请求 Codex 打开原对话")?;
        if !status.success() {
            return Err("Codex 无法打开原对话".into());
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = home;
        Err("当前系统尚未支持自动加载原桌面对话".into())
    }
}
