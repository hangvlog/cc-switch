use serde::Serialize;
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

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

#[tauri::command]
pub fn get_codex_remote_status(
    state: State<'_, CodexRemoteState>,
) -> Result<CodexServerStatus, String> {
    let mut process = state.0.lock().map_err(|error| error.to_string())?;
    Ok(server_status(&mut process))
}

#[tauri::command]
pub fn get_clawkit_codex_configuration_status(
) -> crate::clawkit_codex_config::ClawkitCodexConfigurationStatus {
    crate::clawkit_codex_config::status()
}

#[tauri::command]
pub async fn get_clawkit_codex_model_options(
) -> Result<crate::clawkit_codex_config::ClawkitCodexModelOptions, String> {
    let gateway = crate::clawkit_gateway::bootstrap().await?;
    crate::clawkit_codex_config::model_options(&gateway)
}

#[tauri::command]
pub async fn configure_clawkit_codex(
    selected_model: Option<String>,
) -> Result<crate::clawkit_codex_config::ClawkitCodexConfigurationStatus, String> {
    let gateway = crate::clawkit_gateway::bootstrap().await?;
    crate::clawkit_codex_config::apply(&gateway, selected_model.as_deref())
}

#[tauri::command]
pub async fn upload_clawkit_diagnostic_bundle(
) -> Result<crate::diagnostic_bundle::DiagnosticBundleUpload, String> {
    crate::diagnostic_bundle::create_and_upload().await
}

#[tauri::command]
pub fn rollback_clawkit_codex_configuration(
) -> Result<crate::clawkit_codex_config::ClawkitCodexConfigurationStatus, String> {
    crate::clawkit_codex_config::rollback()
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

    let configuration = crate::clawkit_codex_config::status();
    if !configuration.configured {
        return Err("请先完成 Codex 一键配置".to_string());
    }
    let default_model = configuration
        .model
        .clone()
        .ok_or_else(|| "Codex 配置缺少默认模型".to_string())?;
    let codex_home = validate_codex_home(codex_home_override)?;
    let binary = super::remote_codex_cli::find_codex_cli(codex_home.as_deref()).ok_or_else(|| {
        "手机远程需要 Codex app-server，但未找到 Codex Desktop 内置 CLI 或全局 Codex CLI；一键配置和桌面端使用不受影响".to_string()
    })?;
    let mut command = super::remote_codex_cli::codex_command(&binary);
    command
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    if let Some(path) = codex_home {
        command.env("CODEX_HOME", path);
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("Unable to start Codex app-server: {error}"))?;
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
            models: configuration.models.clone(),
            available_quota: configuration.available_quota.unwrap_or_default(),
            used_quota: configuration.used_quota.unwrap_or_default(),
        };
        let _ = duplicate.child.kill();
        return Ok(server_status(&mut process));
    }
    *process = Some(ServerProcess {
        child,
        stdin,
        model: default_model,
        models: configuration.models,
        available_quota: configuration.available_quota.unwrap_or_default(),
        used_quota: configuration.used_quota.unwrap_or_default(),
    });
    Ok(server_status(&mut process))
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
    codex_plus_plus_status(find_codex_plus_plus_binary())
}

fn codex_plus_plus_status(binary: Option<PathBuf>) -> CodexPlusPlusStatus {
    match binary {
        // Presence checks must stay side-effect free. The launcher does not expose a
        // `status` subcommand, so executing it here would launch Codex while merely
        // opening ClawKit Desktop.
        Some(_) => CodexPlusPlusStatus {
            installed: true,
            summary: "ClawKit Codex 增强层已就绪".to_string(),
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
        if let Some(_directory) = current.parent() {
            #[cfg(target_os = "windows")]
            {
                let sibling = _directory.join("codex-plus-plus.exe");
                if sibling.exists() {
                    return Some(sibling);
                }
            }
            #[cfg(target_os = "macos")]
            {
                let bundled = _directory.join("codex-plus-plus");
                if bundled.exists() {
                    return Some(bundled);
                }
                if let Some(applications) = _directory
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
    find_codex_plus_plus_on_path(std::env::var_os("PATH"))
}

fn find_codex_plus_plus_on_path(path: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let path = path?;
    let names: &[&str] = if cfg!(target_os = "windows") {
        &["codex-plus-plus.exe", "codexplusplus.exe"]
    } else {
        &["codex-plus-plus", "codexplusplus"]
    };
    std::env::split_paths(&path)
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::{codex_plus_plus_status, find_codex_plus_plus_on_path, validate_codex_home};

    #[test]
    fn rejects_relative_test_config_roots() {
        assert!(validate_codex_home(Some("relative/path".into())).is_err());
    }

    #[test]
    fn finds_launcher_on_path_without_executing_it() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let binary_name = if cfg!(target_os = "windows") {
            "codex-plus-plus.exe"
        } else {
            "codex-plus-plus"
        };
        let binary = temp.path().join(binary_name);
        std::fs::write(&binary, b"must not be executed").expect("write fake launcher");

        let found = find_codex_plus_plus_on_path(Some(temp.path().as_os_str().to_os_string()));

        assert_eq!(found.as_deref(), Some(binary.as_path()));
    }

    #[test]
    fn status_check_does_not_execute_the_launcher() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let marker = temp.path().join("launched");
        let binary = temp.path().join("codex-plus-plus");
        std::fs::write(&binary, format!("touch {}", marker.display()))
            .expect("write fake launcher");

        let status = codex_plus_plus_status(Some(binary));

        assert!(status.installed);
        assert_eq!(status.summary, "ClawKit Codex 增强层已就绪");
        assert!(!marker.exists());
    }
}
