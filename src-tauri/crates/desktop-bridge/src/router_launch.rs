use serde_json::Value;
use std::{
    os::unix::{fs::MetadataExt, net::UnixStream},
    path::{Path, PathBuf},
    time::Duration,
};
pub fn maybe_run() {
    if std::env::args().nth(1).as_deref() != Some("--clawkit-ipc-router") {
        return;
    }
    let result = std::env::args_os()
        .nth(2)
        .map(PathBuf::from)
        .ok_or("Missing home".to_owned())
        .and_then(|home| {
            let keeper = std::env::args().nth(3).and_then(|v| v.parse::<i32>().ok()).filter(|v| *v > 1);
            crate::router::run(&home, keeper)
        });
    std::process::exit(if result.is_ok() { 0 } else { 2 });
}

pub fn ensure_before_launch(home: &Path, executable: &Path) -> Result<(), String> {
    let path = home.join("ipc/ipc.sock");
    if path.exists() {
        if UnixStream::connect(&path).is_ok() {
            let marker = std::fs::read(home.join("ipc/clawkit-router.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
            if marker.as_ref().is_some_and(|m| {
                m["version"] == 1
                    && std::fs::symlink_metadata(&path)
                        .is_ok_and(|meta| m["inode"].as_u64() == Some(meta.ino()))
                    && m["pid"].as_i64().is_some_and(|pid| {
                        pid > 0
                            && pid <= i32::MAX as i64
                            && unsafe { libc::kill(pid as i32, 0) } == 0
                    })
            }) {
                return Ok(());
            }
            return Err("已有 Codex IPC 进程，请正常退出 Codex 后再启动".into());
        }
        use std::os::unix::fs::FileTypeExt;
        let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if !meta.file_type().is_socket() || meta.uid() != unsafe { libc::geteuid() } {
            return Err("现有 IPC 路径不属于当前用户".into());
        }
        std::fs::rename(
            &path,
            path.with_file_name(format!("ipc.sock.before-clawkit-{}", uuid::Uuid::new_v4())),
        )
        .map_err(|e| e.to_string())?;
    }
    use std::process::{Command, Stdio};
    let mut child = Command::new(executable)
        .arg("--clawkit-ipc-router")
        .arg(home)
        .env_remove("CODEX_CLI_PATH")
        .env_remove("CLAWKIT_CODEX_ENGINE")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    for _ in 0..30 {
        if UnixStream::connect(&path).is_ok() {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok(());
        }
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            return Err("统一 IPC 路由未能启动".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    Err("统一 IPC 路由启动超时".into())
}

/// Keep the same-user router ready before a future ordinary app launch. An
/// existing official endpoint is always left alone, including startup races.
pub fn maintain(home: &Path, executable: &Path) -> Result<(), String> {
    let path = home.join("ipc/ipc.sock");
    if UnixStream::connect(&path).is_ok() { return Ok(()); }
    if path.exists() {
        use std::os::unix::fs::FileTypeExt;
        let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if !meta.file_type().is_socket() || meta.uid() != unsafe { libc::geteuid() } {
            return Err("IPC path is not an owned socket".into());
        }
        // A bound socket may not be listening yet. Leave any process-owned
        // endpoint alone, even if connect reports ECONNREFUSED during startup.
        let owners = std::process::Command::new("/usr/sbin/lsof")
            .args(["-a","-U","-Fp"]).arg(&path).output().map_err(|e|e.to_string())?;
        if !owners.status.success() && owners.status.code()!=Some(1) {
            return Err("Cannot inspect IPC listener".into());
        }
        if String::from_utf8_lossy(&owners.stdout).lines().any(|v|v.starts_with('p')) {
            return Ok(());
        }
        // Do not remove a listener that has appeared since the first probe.
        if UnixStream::connect(&path).is_ok() { return Ok(()); }
        if !std::fs::symlink_metadata(&path).is_ok_and(|now|now.ino()==meta.ino()) {
            return Ok(());
        }
        std::fs::rename(&path,path.with_file_name(format!("ipc.sock.before-clawkit-{}",uuid::Uuid::new_v4())))
            .map_err(|e|e.to_string())?;
    }
    let mut child = std::process::Command::new(executable)
        .arg("--clawkit-ipc-router").arg(home).arg(std::process::id().to_string())
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null()).spawn().map_err(|e|e.to_string())?;
    std::thread::spawn(move || { let _ = child.wait(); });
    Ok(())
}
