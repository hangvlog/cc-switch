//! Hand a finished phone-created CLI session to the Claude desktop. The desktop
//! imports it through its own `claude://resume` link, the same one the CLI uses
//! to continue a terminal session there, so the session shows up in its list.
use std::process::{Command, Stdio};

pub fn resume_url(cli_id: &str) -> Option<String> {
    super::index::is_uuid(cli_id).then(|| format!("claude://resume?session={cli_id}"))
}

/// Opens the link without bringing the desktop to the foreground.
pub fn open(cli_id: &str) -> Result<(), String> {
    let url = resume_url(cli_id).ok_or("Claude 会话 ID 无效")?;
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = Command::new("/usr/bin/open");
        c.args(["-g", &url]);
        c
    };
    #[cfg(windows)]
    let mut command = {
        let mut c = Command::new("rundll32.exe");
        c.args(["url.dll,FileProtocolHandler", &url]);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut c = Command::new("xdg-open");
        c.arg(&url);
        c
    };
    let status = command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status()
        .map_err(|_| "无法通知 Claude 桌面端")?;
    status.success().then_some(()).ok_or_else(|| "Claude 桌面端未接受会话导入".into())
}
