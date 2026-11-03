//! Deliver a phone message into the Claude process the desktop already runs
//! for a session, through that process's cross-session inbox
//! (`messagingSocketPath` in `~/.claude/sessions/<pid>.json`). The desktop
//! keeps a process per loaded session, so this is the only way to reach an
//! open session without a second writer forking the transcript.
use super::index::{Live, Paths};
use serde_json::{json, Value};
use std::{fs, path::Path, time::Duration};

/// Shown as the sender in the transcript; `projection` recognises it.
pub const FROM: &str = "ClawKit 手机";

/// `<pid>.<64 hex>.key` holds the `peerToken` other local processes present.
fn peer_token(paths: &Paths, pid: i64) -> Option<String> {
    let prefix = format!("{pid}.");
    fs::read_dir(paths.claude_home.join("sessions")).ok()?.flatten().find_map(|entry| {
        let name = entry.file_name().to_string_lossy().into_owned();
        let hash = name.strip_prefix(&prefix)?.strip_suffix(".key")?;
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let key: Value = serde_json::from_slice(&fs::read(entry.path()).ok()?).ok()?;
        key["peerToken"].as_str().filter(|t| t.len() == 32 && t.chars().all(|c| c.is_ascii_hexdigit())).map(String::from)
    })
}

/// The transcript record the inbox writes takes `uuid` from the message, so the
/// request id doubles as a delivery receipt.
pub fn message_line(request_id: &str, text: &str) -> String {
    json!({"type":"user","from":FROM,"uuid":request_id,"message":{"role":"user","content":text}}).to_string()
}

#[cfg(unix)]
pub fn deliver(paths: &Paths, live: &Live, request_id: &str, text: &str) -> Result<(), String> {
    use std::{io::Write, os::unix::net::UnixStream};
    let socket = live.socket.as_deref().ok_or("电脑上的 Claude 版本不支持接收手机消息，请在电脑上更新 Claude")?;
    let mut stream = UnixStream::connect(socket).map_err(|_| "无法连接电脑上的 Claude 会话，请稍后重试")?;
    stream.set_write_timeout(Some(Duration::from_secs(3))).ok();
    let mut payload = String::new();
    if let Some(token) = peer_token(paths, live.pid) {
        payload.push_str(&json!({"type":"auth","token":token}).to_string());
        payload.push('\n');
    }
    payload.push_str(&message_line(request_id, text));
    payload.push('\n');
    stream.write_all(payload.as_bytes()).and_then(|_| stream.flush()).map_err(|_| "消息未能送达电脑上的 Claude 会话".to_string())?;
    // Give the inbox time to read before the connection closes.
    std::thread::sleep(Duration::from_millis(300));
    Ok(())
}

#[cfg(not(unix))]
pub fn deliver(_: &Paths, _: &Live, _: &str, _: &str) -> Result<(), String> {
    Err("此会话正在电脑上打开，暂不能从手机发送".into())
}

/// Whether the record with this uuid has reached the end of the transcript.
pub fn recorded(transcript: &Path, request_id: &str) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    const TAIL: u64 = 2 * 1024 * 1024;
    let Ok(mut file) = fs::File::open(transcript) else { return false };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut tail = Vec::new();
    if file.seek(SeekFrom::Start(len.saturating_sub(TAIL))).is_err() || file.read_to_end(&mut tail).is_err() {
        return false;
    }
    // `uuid` when it starts a turn, `source_uuid` when queued into a running one.
    let needle = format!("uuid\":\"{request_id}\"");
    String::from_utf8_lossy(&tail).contains(&needle)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Inbound {
    Accept,
    /// Claude parks the message until you approve it on the desktop.
    Hold,
    Refuse,
}

fn setting(path: &Path) -> Option<String> {
    let value: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    value["crossSessionInbound"].as_str().map(String::from)
}

/// What the receiving CLI does with a message from another local process,
/// following its own `crossSessionInbound` rules: managed settings win, a
/// repository may only tighten, then your user settings; without a setting, a
/// session that skips permission prompts holds messages whose sender did not
/// attest a matching permission mode.
pub fn inbound(paths: &Paths, cwd: &str, permission_mode: Option<&str>) -> Inbound {
    let parse = |v: &str| match v {
        "accept" => Some(Inbound::Accept),
        "hold" => Some(Inbound::Hold),
        "refuse" => Some(Inbound::Refuse),
        _ => Some(Inbound::Hold),
    };
    let managed = [Path::new("/Library/Application Support/ClaudeCode/managed-settings.json"), Path::new("/etc/claude-code/managed-settings.json")];
    if let Some(value) = managed.iter().find_map(|p| setting(p)) {
        return parse(&value).unwrap_or(Inbound::Hold);
    }
    let repo = Path::new(cwd).join(".claude/settings.json");
    if let Some(value) = setting(&repo).filter(|v| v != "accept") {
        return parse(&value).unwrap_or(Inbound::Hold);
    }
    let user = [paths.claude_home.join("settings.local.json"), paths.claude_home.join("settings.json"), Path::new(cwd).join(".claude/settings.local.json")];
    if let Some(value) = user.iter().find_map(|p| setting(p)) {
        return parse(&value).unwrap_or(Inbound::Hold);
    }
    if permission_mode == Some("bypassPermissions") { Inbound::Hold } else { Inbound::Accept }
}

pub fn blocked_reason(inbound: Inbound) -> Option<&'static str> {
    match inbound {
        Inbound::Accept => None,
        Inbound::Hold => Some("这个会话在电脑上跳过权限确认，Claude 会把手机消息扣下等你在电脑上确认。在 Claude 设置（~/.claude/settings.json）加入 \"crossSessionInbound\": \"accept\" 后可直接发送"),
        Inbound::Refuse => Some("Claude 设置拒绝接收其他程序发来的消息（crossSessionInbound: refuse），请在电脑上继续"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(home: &Path) -> Paths {
        Paths { claude_home: home.to_path_buf(), app_data: None, registry: home.join("r.json"), cli_override: None }
    }

    #[test]
    fn follows_cross_session_inbound_rules() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let repo = dir.path().join("repo");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(repo.join(".claude")).unwrap();
        let cwd = repo.to_str().unwrap();
        let p = paths(&home);
        assert_eq!(inbound(&p, cwd, Some("bypassPermissions")), Inbound::Hold);
        assert_eq!(inbound(&p, cwd, Some("acceptEdits")), Inbound::Accept);
        fs::write(home.join("settings.json"), r#"{"crossSessionInbound":"accept"}"#).unwrap();
        assert_eq!(inbound(&p, cwd, Some("bypassPermissions")), Inbound::Accept);
        fs::write(repo.join(".claude/settings.json"), r#"{"crossSessionInbound":"hold"}"#).unwrap();
        assert_eq!(inbound(&p, cwd, Some("acceptEdits")), Inbound::Hold, "a repository may tighten");
        fs::write(repo.join(".claude/settings.json"), r#"{"crossSessionInbound":"accept"}"#).unwrap();
        fs::write(home.join("settings.json"), r#"{"crossSessionInbound":"refuse"}"#).unwrap();
        assert_eq!(inbound(&p, cwd, Some("acceptEdits")), Inbound::Refuse);
    }
}
