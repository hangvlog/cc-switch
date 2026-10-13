use serde_json::{json, Value};
use std::path::Path;

// Build numbers are diagnostic only. Transport, snapshot and operation schemas
// are checked independently so unrelated desktop updates do not disable access.

#[cfg(target_os = "macos")]
pub(crate) fn running_pid(home: &Path) -> Option<i32> {
    use std::process::Command;
    let output = Command::new("/usr/sbin/lsof")
        .args(["-a", "-U", "-Fp"]).arg(home.join("ipc/ipc.sock")).output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let mut pids: Vec<i32> = text.lines().filter_map(|s|s.strip_prefix('p')?.parse().ok()).collect();
    // When our router owns the pathname, lsof lists only the router's end of
    // connected sockets. Its peer PID list comes from LOCAL_PEERPID, never a
    // client-supplied initialize field.
    use std::os::unix::fs::MetadataExt;
    if let Some(marker)=std::fs::read(home.join("ipc/clawkit-router.json")).ok()
        .and_then(|v|serde_json::from_slice::<Value>(&v).ok()) {
        if marker["pid"].as_i64().is_some_and(|pid|pids.contains(&(pid as i32)))
            && std::fs::symlink_metadata(home.join("ipc/ipc.sock")).is_ok_and(|m|marker["inode"].as_u64()==Some(m.ino())) {
            pids.extend(marker["peerPids"].as_array().into_iter().flatten()
                .filter_map(|v|v.as_i64().and_then(|n|i32::try_from(n).ok())));
        }
    }
    pids.into_iter().find(|pid|{
        Command::new("/bin/ps").args(["-p",&pid.to_string(),"-o","comm="]).output().ok()
            .and_then(|o|String::from_utf8(o.stdout).ok()).is_some_and(|s|{
                s.trim().ends_with("/ChatGPT.app/Contents/MacOS/ChatGPT") || s.trim().ends_with("/Codex.app/Contents/MacOS/Codex")
            })
    })
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
    #[cfg(target_os="macos")]
    if home.join("ipc/clawkit-router.json").exists() && running_pid(home).is_none() {
        return crate::native::event_ready(home);
    }
    crate::native::event_ready(home) || crate::ipc::IpcClient::connect(home).is_ok()
}

pub fn status(home: &Path) -> Value {
    let build = running_build(home);
    #[cfg(target_os="macos")]
    let activity=crate::renderer::request(home,"status",None).unwrap_or_else(|reason|json!({"ready":false,"reason":reason}));
    #[cfg(not(target_os="macos"))]
    let activity=json!({"ready":false,"reason":"此系统尚未支持官方活动视图同步"});
    json!({"mode":"desktop-owner","build":build,"activity":activity,"compatibility":"operation-protocol",
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
