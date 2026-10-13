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
        .and_then(|home| crate::router::run(&home));
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
