//! Same-user launchd service. No window, Codex restart, token copy or second engine.
use super::{codex_home, preference, storage, ClawkitAccountClient, DesktopRemoteState};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

static STORAGE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
const VERSION: &str = env!("CARGO_PKG_VERSION");
pub(super) fn storage_override() -> Option<PathBuf> {
    STORAGE.get().cloned()
}
fn enabled() -> bool {
    fs::read_to_string(preference()).ok().as_deref() == Some("true")
}
fn label() -> String {
    format!(
        "chat.clawkit.remote.{:x}",
        Sha256::digest(storage().as_os_str().as_encoded_bytes())
    )
}
fn domain() -> String {
    format!("gui/{}", unsafe { libc::geteuid() })
}
fn target() -> String {
    format!("{}/{}", domain(), label())
}
fn plist_path() -> PathBuf {
    crate::config::get_home_dir()
        .join("Library/LaunchAgents")
        .join(format!("{}.plist", label()))
}
fn private_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|_| "无法创建后台服务目录")?;
    let m = fs::symlink_metadata(path).map_err(|_| "后台目录不可用")?;
    if !m.is_dir() || m.file_type().is_symlink() || m.uid() != unsafe { libc::geteuid() } {
        return Err("后台目录不属于当前用户".into());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| "无法保护后台目录".into())
}
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut f = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|_| "无法写入后台配置")?;
    let result = f
        .write_all(bytes)
        .and_then(|_| f.sync_all())
        .and_then(|_| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result.map_err(|_| "无法保存后台配置".into())
}
fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn plist(exe: &Path) -> String {
    let mut env = vec![(
        "CODEX_HOME".to_string(),
        codex_home().to_string_lossy().into_owned(),
    )];
    // Explicit non-secret endpoint/profile configuration only. Credentials stay in the SSO file.
    for name in [
        "CC_SWITCH_TEST_HOME",
        "CLAWKIT_ACCOUNT_API_BASE_URL",
        "CLAWKIT_RELAY_API_BASE_URL",
        "CLAWKIT_API_BASE_URL",
        "CLAWKIT_LOG_ENDPOINT",
    ] {
        if let Ok(value) = std::env::var(name) {
            env.push((name.into(), value));
        }
    }
    let environment = env
        .into_iter()
        .map(|(k, v)| format!("<key>{}</key><string>{}</string>", xml(&k), xml(&v)))
        .collect::<String>();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict>
<key>Label</key><string>{}</string><key>ProgramArguments</key><array><string>{}</string><string>--clawkit-remote-service</string><string>{}</string></array>
<key>RunAtLoad</key><true/><key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict><key>ThrottleInterval</key><integer>10</integer><key>ProcessType</key><string>Background</string><key>EnvironmentVariables</key><dict>{}</dict></dict></plist>"#,
        xml(&label()),
        xml(&exe.to_string_lossy()),
        xml(&storage().to_string_lossy()),
        environment
    )
}
fn ctl(args: &[&str]) -> bool {
    Command::new("/bin/launchctl")
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}
fn heartbeat() -> Option<Value> {
    let path = storage().join("service-status.json");
    let m = fs::symlink_metadata(&path).ok()?;
    if !m.is_file()
        || m.file_type().is_symlink()
        || m.uid() != unsafe { libc::geteuid() }
        || m.len() > 8192
    {
        return None;
    }
    let v: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let age = clawkit_desktop_bridge::now_ms().checked_sub(v["updatedAt"].as_u64()?)?;
    let pid = v["pid"].as_i64()?;
    if age > 5000 || pid <= 1 || pid > i32::MAX as i64 || unsafe { libc::kill(pid as i32, 0) } != 0
    {
        return None;
    }
    Some(v)
}
pub(super) fn status() -> Result<Value, String> {
    let wanted = enabled();
    let mut v = heartbeat().unwrap_or_else(
        || json!({"phase":if wanted{"reconnecting"}else{"disabled"},"serviceRunning":false}),
    );
    v["enabled"] = json!(wanted);
    v["mode"] = json!("desktop-owner");
    v["backgroundService"] = json!(true);
    if !wanted {
        v["phase"] = json!("disabled");
    }
    Ok(v)
}
fn start() -> Result<(), String> {
    private_dir(&storage())?;
    let exe = std::env::current_exe()
        .map_err(|_| "无法定位后台程序")?
        .canonicalize()
        .map_err(|_| "后台程序不存在")?;
    if exe.to_string_lossy().starts_with("/Volumes/") {
        return Err("请先将 ClawKit Desktop 放入应用程序目录，再启用手机远程".into());
    }
    let path = plist_path();
    fs::create_dir_all(path.parent().unwrap()).map_err(|_| "无法保存登录启动配置")?;
    let text = plist(&exe);
    let unchanged = fs::read_to_string(&path).ok().as_deref() == Some(text.as_str());
    let loaded = ctl(&["print", &target()]);
    let current = heartbeat();
    if loaded && unchanged && current.as_ref().is_some_and(|v| v["version"] == VERSION) {
        return Ok(());
    }
    if loaded && !ctl(&["bootout", &target()]) {
        return Err("旧后台服务尚未退出，请稍后重试".into());
    }
    write_private(&path, text.as_bytes())?;
    if !ctl(&["bootstrap", &domain(), &path.to_string_lossy()]) {
        return Err("系统未能启动远程后台服务".into());
    }
    for _ in 0..50 {
        if heartbeat().is_some_and(|v| v["version"] == VERSION && v["serviceRunning"] == true) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("后台服务启动未确认，可稍后在设置中重试".into())
}
pub(super) fn set_enabled(value: bool) -> Result<Value, String> {
    private_dir(&storage())?;
    if value {
        ClawkitAccountClient::default().active_credentials()?;
    }
    write_private(&preference(), if value { b"true" } else { b"false" })?;
    if value {
        if let Err(e) = start() {
            let _ = write_private(&preference(), b"false");
            let _ = ctl(&["bootout", &target()]);
            return Err(e);
        }
    } else {
        // Preference is checked in the daemon; invalidate dispatch before unloading it.
        for _ in 0..30 {
            if heartbeat().is_none_or(|v| v["serviceRunning"] != true) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if ctl(&["print", &target()]) && !ctl(&["bootout", &target()]) {
            return Err("停止请求已保存，后台卸载尚未确认".into());
        }
        let path = plist_path();
        if path.exists() {
            fs::remove_file(path).map_err(|_| "远程已停用，但登录启动配置移除失败")?;
        }
    }
    status()
}
pub(super) fn can_fallback() -> bool {
    !ctl(&["print", &target()]) && heartbeat().is_none_or(|v| v["serviceRunning"] != true)
}
pub(super) fn restore() -> Result<(), String> {
    if enabled() {
        start()
    } else {
        Ok(())
    }
}
fn daemon() -> Result<(), String> {
    private_dir(&storage())?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(storage().join("service.lock"))
        .map_err(|_| "后台服务锁不可用")?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Ok(());
    }
    let state = DesktopRemoteState::default();
    let persist = |running: bool| {
        let mut v = state
            .status()
            .unwrap_or_else(|_| json!({"phase":"reconnecting"}));
        v["pid"] = json!(std::process::id());
        v["version"] = json!(VERSION);
        v["serviceRunning"] = json!(running);
        v["updatedAt"] = json!(clawkit_desktop_bridge::now_ms());
        write_private(
            &storage().join("service-status.json"),
            v.to_string().as_bytes(),
        )
    };
    tauri::async_runtime::block_on(async {
        let mut retry = 0;
        while enabled() {
            if retry == 0 {
                if state.start(None).is_err() {
                    super::phase(None, &state, "login-required");
                }
                retry = 30;
            }
            persist(true)?;
            retry -= 1;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        state.stop()?;
        persist(false)?;
        Ok(())
    })
}
pub(super) fn maybe_run() {
    // The GUI normally installs this in setup; the headless entry skips setup.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args: Vec<_> = std::env::args_os().collect();
    let Some(flag) = args.get(1).and_then(|v| v.to_str()) else {
        return;
    };
    let result = match flag {
        "--clawkit-remote-service" => {
            let Some(path) = args.get(2).map(PathBuf::from).filter(|p| p.is_absolute()) else {
                std::process::exit(2)
            };
            let _ = STORAGE.set(path);
            daemon().map(|_| json!({"stopped":true}))
        }
        "--clawkit-remote-enable" => set_enabled(true),
        "--clawkit-remote-disable" => set_enabled(false),
        "--clawkit-remote-status" => status(),
        _ => return,
    };
    match result {
        Ok(v) => {
            println!("{v}");
            std::process::exit(0)
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn launchd_arguments_are_escaped_and_do_not_embed_authentication() {
        let text = plist(Path::new(
            "/Applications/A & B.app/Contents/MacOS/cc-switch",
        ));
        assert!(text.contains("A &amp; B.app"));
        assert!(text.contains("--clawkit-remote-service"));
        assert!(!text.contains("Bearer"));
        assert!(!text.contains("clawkit-account.json"));
        assert!(text.contains("<key>SuccessfulExit</key><false/>"));
    }
}
