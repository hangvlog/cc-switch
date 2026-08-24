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
    let binary = std::env::var("CODEX_REMOTE_CODEX_BIN").unwrap_or_else(|_| "codex".into());
    let mut command = Command::new(binary);
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
    match find_codex_plus_plus_binary().and_then(|binary| {
        Command::new(binary)
            .arg("status")
            .stdin(Stdio::null())
            .output()
            .ok()
    }) {
        Some(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            CodexPlusPlusStatus {
                installed: output.status.success() && !stdout.contains("Not installed"),
                summary: if stdout.is_empty() { stderr } else { stdout },
            }
        }
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
    use super::{toml_override, validate_codex_home};

    #[test]
    fn rejects_relative_test_config_roots() {
        assert!(validate_codex_home(Some("relative/path".into())).is_err());
    }

    #[test]
    fn gateway_secret_stays_out_of_codex_arguments() {
        assert_eq!(
            toml_override("model_providers.clawkit.env_key", "CLAWKIT_CODEX_API_KEY"),
            r#"model_providers.clawkit.env_key=\"CLAWKIT_CODEX_API_KEY\""#
        );
    }
}
