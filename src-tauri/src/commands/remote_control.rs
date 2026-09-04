use serde::Serialize;
use serde_json::Value;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct ServerProcess {
    child: Child,
    stdin: ChildStdin,
    model: String,
    models: Vec<String>,
    available_quota: i64,
    used_quota: i64,
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Default)]
pub struct CodexRemoteState(Mutex<Option<ServerProcess>>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexServerStatus {
    running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    models: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    available_quota: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    used_quota: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexPlusPlusStatus {
    installed: bool,
    summary: String,
}

fn server_status(process: &mut Option<ServerProcess>) -> CodexServerStatus {
    let exited = process
        .as_mut()
        .and_then(|server| server.child.try_wait().ok().flatten())
        .is_some();
    if exited {
        process.take();
    }
    CodexServerStatus {
        running: process.is_some(),
        model: process.as_ref().map(|server| server.model.clone()),
        models: process
            .as_ref()
            .map(|server| server.models.clone())
            .unwrap_or_default(),
        available_quota: process.as_ref().map(|server| server.available_quota),
        used_quota: process.as_ref().map(|server| server.used_quota),
    }
}

fn validate_codex_home(value: Option<String>) -> Result<Option<PathBuf>, String> {
    let Some(raw) = value else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err("CODEX_HOME override must be an absolute path".to_string());
    }
    std::fs::create_dir_all(&path).map_err(|error| error.to_string())?;
    Ok(Some(path))
}

fn codex_binary_names() -> &'static [&'static str] {
    // npm installs the Windows entry point as `codex.cmd`; CreateProcess only
    // resolves `.exe` from a bare name, so each directory is probed for these
    // file names explicitly.
    #[cfg(target_os = "windows")]
    {
        &["codex.exe", "codex.cmd", "codex.bat"]
    }
    #[cfg(not(target_os = "windows"))]
    {
        &["codex"]
    }
}

fn find_codex_in_dir(directory: &Path) -> Option<PathBuf> {
    codex_binary_names()
        .iter()
        .map(|name| directory.join(name))
        .find(|candidate| candidate.is_file())
}

fn find_codex_on_path(path_value: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path_value)
        .filter(|directory| !directory.as_os_str().is_empty())
        .find_map(|directory| find_codex_in_dir(&directory))
}

/// Locates the Codex CLI without assuming it is reachable through the app's
/// PATH: a GUI process launched from Finder or Explorer only inherits a
/// minimal PATH, so a bare `Command::new("codex")` fails even on machines
/// where the CLI is installed.
fn find_codex_binary() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("CODEX_REMOTE_CODEX_BIN") {
        let explicit = PathBuf::from(explicit);
        if explicit.is_file() {
            return Some(explicit);
        }
        log::warn!(
            "CODEX_REMOTE_CODEX_BIN points to a missing file: {}",
            explicit.display()
        );
    }

    if let Ok(current) = std::env::current_exe() {
        if let Some(directory) = current.parent() {
            if let Some(bundled) = find_codex_in_dir(directory) {
                return Some(bundled);
            }
        }
    }

    if let Some(path_value) = std::env::var_os("PATH") {
        if let Some(found) = find_codex_on_path(&path_value) {
            return Some(found);
        }
    }

    crate::codex_config::codex_cli_candidates()
        .into_iter()
        .find(|candidate| candidate.is_absolute() && candidate.is_file())
}

#[tauri::command]
pub fn get_codex_remote_status(
    state: State<'_, CodexRemoteState>,
) -> Result<CodexServerStatus, String> {
    let mut process = state.0.lock().map_err(|error| error.to_string())?;
    Ok(server_status(&mut process))
}

#[tauri::command]
pub async fn start_codex_remote_server(
    codex_home_override: Option<String>,
    app: AppHandle,
    state: State<'_, CodexRemoteState>,
) -> Result<CodexServerStatus, String> {
    {
        let mut process = state.0.lock().map_err(|error| error.to_string())?;
        let current = server_status(&mut process);
        if current.running {
            return Ok(current);
        }
    }

    let gateway = crate::clawkit_gateway::bootstrap().await?;
    let catalog_path = crate::clawkit_gateway::write_model_catalog(&gateway.models)?;
    let default_model = crate::clawkit_gateway::preferred_default_model(&gateway.models)
        .ok_or_else(|| "当前账号没有可用的 API 模型".to_string())?
        .to_string();
    let codex_home = validate_codex_home(codex_home_override)?;
    let binary = find_codex_binary().ok_or_else(|| {
        "未找到 Codex CLI：请先安装 Codex（npm install -g @openai/codex），\
         或设置环境变量 CODEX_REMOTE_CODEX_BIN 指向 codex 可执行文件"
            .to_string()
    })?;
    let mut command = Command::new(&binary);
    // A release build uses the Windows GUI subsystem; without this flag a
    // console child (especially `codex.cmd` via cmd.exe) opens its own
    // transient console window.
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        .arg("-c")
        .arg(toml_override("model_provider", "clawkit"))
        .arg("-c")
        .arg(toml_override("model", &default_model))
        .arg("-c")
        .arg(toml_override(
            "model_catalog_json",
            catalog_path.to_string_lossy().as_ref(),
        ))
        .arg("-c")
        .arg(toml_override("model_providers.clawkit.name", "ClawKit API"))
        .arg("-c")
        .arg(toml_override(
            "model_providers.clawkit.base_url",
            &gateway.base_url,
        ))
        .arg("-c")
        .arg(toml_override(
            "model_providers.clawkit.env_key",
            "CLAWKIT_CODEX_API_KEY",
        ))
        .arg("-c")
        .arg(toml_override(
            "model_providers.clawkit.wire_api",
            "responses",
        ))
        .arg("-c")
        .arg("model_providers.clawkit.requires_openai_auth=false")
        .arg("app-server")
        .env("CLAWKIT_CODEX_API_KEY", &gateway.api_key)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = codex_home {
        command.env("CODEX_HOME", path);
    }

    let mut child = command.spawn().map_err(|error| {
        format!(
            "无法启动 Codex app-server（{}）：{error}",
            binary.display()
        )
    })?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Codex app-server stdin is unavailable".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Codex app-server stdout is unavailable".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Codex app-server stderr is unavailable".to_string())?;

    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(payload) => {
                    if let Err(error) = app.emit("codex-remote-message", payload) {
                        log::warn!("Unable to emit Codex remote message: {error}");
                    }
                }
                Err(error) => {
                    log::warn!("Unable to read Codex app-server output: {error}");
                    break;
                }
            }
        }
    });

    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            match line {
                Ok(payload) => log::warn!("Codex app-server: {payload}"),
                Err(error) => {
                    log::warn!("Unable to read Codex app-server error output: {error}");
                    break;
                }
            }
        }
    });

    let mut process = state.0.lock().map_err(|error| error.to_string())?;
    if server_status(&mut process).running {
        let mut duplicate = ServerProcess {
            child,
            stdin,
            model: default_model,
            models: gateway.models,
            available_quota: gateway.available_quota,
            used_quota: gateway.used_quota,
        };
        let _ = duplicate.child.kill();
        return Ok(server_status(&mut process));
    }
    *process = Some(ServerProcess {
        child,
        stdin,
        model: default_model,
        models: gateway.models,
        available_quota: gateway.available_quota,
        used_quota: gateway.used_quota,
    });
    Ok(server_status(&mut process))
}

fn toml_override(key: &str, value: &str) -> String {
    format!(
        "{key}={}",
        serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
    )
}

#[tauri::command]
pub fn send_codex_remote_message(
    payload: String,
    state: State<'_, CodexRemoteState>,
) -> Result<(), String> {
    let mut process = state.0.lock().map_err(|error| error.to_string())?;
    if !server_status(&mut process).running {
        return Err("Codex app-server is not running".to_string());
    }
    let server = process
        .as_mut()
        .ok_or_else(|| "Codex app-server is not running".to_string())?;
    server
        .stdin
        .write_all(payload.as_bytes())
        .and_then(|_| server.stdin.write_all(b"\n"))
        .and_then(|_| server.stdin.flush())
        .map_err(|error| format!("Unable to send Codex message: {error}"))
}

#[tauri::command]
pub fn stop_codex_remote_server(
    state: State<'_, CodexRemoteState>,
) -> Result<CodexServerStatus, String> {
    let mut process = state.0.lock().map_err(|error| error.to_string())?;
    process.take();
    Ok(server_status(&mut process))
}

#[tauri::command]
pub fn get_clawkit_account_status() -> Value {
    crate::clawkit_account::ClawkitAccountClient::default().status()
}

#[tauri::command]
pub async fn login_clawkit_account(username: String, password: String) -> Result<Value, String> {
    crate::clawkit_account::ClawkitAccountClient::default()
        .login(&username, &password)
        .await
}

#[tauri::command]
pub fn logout_clawkit_account() -> Result<Value, String> {
    crate::clawkit_account::ClawkitAccountClient::default().logout()
}

#[tauri::command]
pub async fn create_clawkit_socket_ticket() -> Result<Value, String> {
    crate::clawkit_account::ClawkitAccountClient::default()
        .create_socket_ticket()
        .await
}

#[tauri::command]
pub fn get_codex_plus_plus_status() -> CodexPlusPlusStatus {
    match find_codex_plus_plus_binary() {
        Some(binary) => match Command::new(binary)
            .arg("status")
            .stdin(Stdio::null())
            .output()
        {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                CodexPlusPlusStatus {
                    // The integrated installer already ships this executable. A fresh
                    // installation may legitimately report "Not installed" before the
                    // first launch, but it is still available and must remain launchable.
                    installed: true,
                    summary: if stdout.is_empty() {
                        if stderr.is_empty() {
                            "ClawKit Codex 增强层已就绪".to_string()
                        } else {
                            stderr
                        }
                    } else {
                        stdout
                    },
                }
            }
            Err(error) => CodexPlusPlusStatus {
                installed: true,
                summary: format!("ClawKit Codex 增强层已就绪（状态读取失败：{error}）"),
            },
        },
        None => CodexPlusPlusStatus {
            installed: false,
            summary: "ClawKit Codex 增强层尚未安装".to_string(),
        },
    }
}

#[tauri::command]
pub fn launch_codex_plus_plus() -> Result<(), String> {
    let binary =
        find_codex_plus_plus_binary().ok_or_else(|| "ClawKit Codex 增强层尚未安装".to_string())?;
    Command::new(binary)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法启动 ClawKit Codex：{error}"))
}

fn find_codex_plus_plus_binary() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("CODEX_PLUSPLUS_BIN") {
        let path = PathBuf::from(explicit);
        if path.exists() {
            return Some(path);
        }
    }
    if let Ok(current) = std::env::current_exe() {
        if let Some(directory) = current.parent() {
            #[cfg(target_os = "windows")]
            {
                let sibling = directory.join("codex-plus-plus.exe");
                if sibling.exists() {
                    return Some(sibling);
                }
            }
            #[cfg(target_os = "macos")]
            {
                let bundled = directory.join("codex-plus-plus");
                if bundled.exists() {
                    return Some(bundled);
                }
                if let Some(applications) = directory
                    .parent()
                    .and_then(|contents| contents.parent())
                    .and_then(|app| app.parent())
                {
                    let sibling = applications
                        .join("ClawKit Codex.app")
                        .join("Contents/MacOS/CodexPlusPlus");
                    if sibling.exists() {
                        return Some(sibling);
                    }
                }
            }
        }
    }
    ["codex-plus-plus", "codexplusplus"]
        .into_iter()
        .map(PathBuf::from)
        .find(|candidate| {
            Command::new(candidate)
                .arg("status")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok()
        })
}

#[cfg(test)]
mod tests {
    use super::{find_codex_in_dir, find_codex_on_path, toml_override, validate_codex_home};
    use std::fs;

    #[test]
    fn finds_codex_inside_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(find_codex_in_dir(dir.path()), None);

        let name = if cfg!(target_os = "windows") {
            "codex.cmd"
        } else {
            "codex"
        };
        let binary = dir.path().join(name);
        fs::write(&binary, b"").expect("write stub");
        assert_eq!(find_codex_in_dir(dir.path()), Some(binary));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn prefers_exe_over_cmd_shim() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("codex.cmd"), b"").expect("write cmd");
        fs::write(dir.path().join("codex.exe"), b"").expect("write exe");
        assert_eq!(
            find_codex_in_dir(dir.path()),
            Some(dir.path().join("codex.exe"))
        );
    }

    #[test]
    fn walks_every_path_entry() {
        let empty = tempfile::tempdir().expect("tempdir");
        let hit = tempfile::tempdir().expect("tempdir");
        let name = if cfg!(target_os = "windows") {
            "codex.exe"
        } else {
            "codex"
        };
        let binary = hit.path().join(name);
        fs::write(&binary, b"").expect("write stub");

        let joined = std::env::join_paths([empty.path(), hit.path()]).expect("join paths");
        assert_eq!(find_codex_on_path(&joined), Some(binary));
    }

    #[test]
    fn rejects_relative_test_config_roots() {
        assert!(validate_codex_home(Some("relative/path".into())).is_err());
    }

    #[test]
    fn gateway_secret_stays_out_of_codex_arguments() {
        assert_eq!(
            toml_override("model_providers.clawkit.env_key", "CLAWKIT_CODEX_API_KEY"),
            r#"model_providers.clawkit.env_key="CLAWKIT_CODEX_API_KEY""#
        );
    }
}
