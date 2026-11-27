//! Claude Desktop downgrades imported CLI sessions. Restore the local user's
//! selection on the single native writer before delivering the phone message.
use super::{adopt, index, permission_mode, Paths};
use serde_json::Value;
use std::{ffi::CString, fs, sync::Mutex, time::Duration};

extern "C" {
    fn clawkit_claude_select_bypass(session_id: *const std::ffi::c_char) -> i32;
}

static SELECTOR: Mutex<()> = Mutex::new(());

fn persisted(paths: &Paths, session: &index::Session) -> Result<bool, String> {
    let fresh = index::resolve(paths, &session.id)?;
    if fresh.archived || fresh.cli_id != session.cli_id || fresh.cwd != session.cwd {
        return Err("Claude 会话已归档或发生变化，消息尚未发送".into());
    }
    Ok(fresh.permission_mode.as_deref() == Some("bypassPermissions"))
}

fn opted_in(paths: &Paths, id: &str) -> bool {
    let Some(base) = paths.app_data.as_ref() else { return false };
    let config = fs::read(base.join("claude_desktop_config.json")).ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    let Some(config) = config else { return false };
    fs::read_dir(base.join("claude-code-sessions")).into_iter().flatten().flatten().any(|account| {
        config["preferences"]["bypassPermissionsOptInByAccount"][account.file_name().to_string_lossy().as_ref()] == true &&
            fs::read_dir(account.path()).into_iter().flatten().flatten()
                .any(|org| org.path().join(format!("{id}.json")).is_file())
    })
}

pub(super) fn ensure(paths: &Paths, session: &index::Session) -> Result<(), String> {
    if session.permission_mode.as_deref() == Some("bypassPermissions") ||
        permission_mode(paths, session.permission_mode.as_deref()) != "bypassPermissions" {
        return Ok(());
    }
    if !session.id.strip_prefix("local_").is_some_and(index::is_uuid) {
        return Err("找不到此会话对应的 Claude 桌面权限选择器".into());
    }
    let _lock = SELECTOR.lock().map_err(|_| "无法访问 Claude 权限选择器")?;
    if persisted(paths, session)? { return Ok(()); }
    if !opted_in(paths, &session.id) {
        return Err("当前 Claude 桌面账户未开启 Bypass permissions。请在 Claude 的 Code 设置中启用后重试".into());
    }
    adopt::open(&session.cli_id)?;
    let id = CString::new(session.id.as_str()).map_err(|_| "Claude 会话编号无效")?;
    // The native helper reads this borrowed C string synchronously.
    match unsafe { clawkit_claude_select_bypass(id.as_ptr()) } {
        0 => (),
        1 => return Err("请在 Mac 系统设置的「隐私与安全性 → 辅助功能」允许 ClawKit Desktop，或在 Claude 中为此会话选择 Bypass permissions".into()),
        2 => return Err("Claude 未显示目标会话。请解锁 Mac，打开此会话，再从手机重试".into()),
        _ => return Err("Claude 未能选择 Bypass permissions。请在该会话的权限菜单核对可用模式".into()),
    }
    for _ in 0..12 {
        if persisted(paths, session)? { return Ok(()); }
        std::thread::sleep(Duration::from_millis(125));
    }
    Err("Claude 尚未保存此会话的 Bypass permissions，消息尚未发送".into())
}
