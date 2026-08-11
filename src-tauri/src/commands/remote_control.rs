use serde::Serialize;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

struct ServerProcess {
    child: Child,
    stdin: ChildStdin,
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
pub fn start_codex_remote_server(
    codex_home_override: Option<String>,
    app: AppHandle,
    state: State<'_, CodexRemoteState>,
) -> Result<CodexServerStatus, String> {
    let mut process = state.0.lock().map_err(|error| error.to_string())?;
    let current = server_status(&mut process);
    if current.running {
        return Ok(current);
    }

    let codex_home = validate_codex_home(codex_home_override)?;
    let binary = std::env::var("CODEX_REMOTE_CODEX_BIN").unwrap_or_else(|_| "codex".into());
    let mut command = Command::new(binary);
    command
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
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

    *process = Some(ServerProcess { child, stdin });
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
pub fn get_codex_plus_plus_status() -> CodexPlusPlusStatus {
    let binary = std::env::var("CODEX_PLUSPLUS_BIN").unwrap_or_else(|_| "codexplusplus".into());
    match Command::new(binary)
        .arg("status")
        .stdin(Stdio::null())
        .output()
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            CodexPlusPlusStatus {
                installed: output.status.success() && !stdout.contains("Not installed"),
                summary: if stdout.is_empty() { stderr } else { stdout },
            }
        }
        Err(_) => CodexPlusPlusStatus {
            installed: false,
            summary: "Codex++ CLI is not available on PATH".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::validate_codex_home;

    #[test]
    fn rejects_relative_test_config_roots() {
        assert!(validate_codex_home(Some("relative/path".into())).is_err());
    }
}
