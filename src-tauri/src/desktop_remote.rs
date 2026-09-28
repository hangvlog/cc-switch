//! Native relay lifetime is independent of the settings WebView.
use crate::clawkit_account::ClawkitAccountClient;
use clawkit_desktop_bridge::Bridge;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio_tungstenite::tungstenite::{protocol::WebSocketConfig, Message};

#[derive(Default)]
struct Runtime {
    task: Option<tauri::async_runtime::JoinHandle<()>>,
    enabled: bool,
    phase: String,
    dispatch_enabled: Option<Arc<std::sync::atomic::AtomicBool>>,
}

#[derive(Clone, Default)]
pub struct DesktopRemoteState(Arc<Mutex<Runtime>>);

fn storage() -> PathBuf {
    crate::config::get_app_config_dir().join("desktop-remote")
}
fn preference() -> PathBuf {
    storage().join("enabled.json")
}
fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::config::get_home_dir().join(".codex"))
}

fn phase(app: &AppHandle, state: &DesktopRemoteState, value: &str) {
    if let Ok(mut runtime) = state.0.lock() {
        if runtime.phase != value {
            crate::desktop_remote_log::report(value);
        }
        runtime.phase = value.into();
        let _ = app.emit(
            "desktop-remote-status",
            json!({"enabled":runtime.enabled,"phase":value}),
        );
    }
}

impl DesktopRemoteState {
    fn status(&self) -> Result<Value, String> {
        let state = self.0.lock().map_err(|_| "远程状态不可用")?;
        Ok(json!({"enabled":state.enabled,"phase":state.phase,"mode":"desktop-owner"}))
    }

    fn stop(&self) -> Result<(), String> {
        let mut state = self.0.lock().map_err(|_| "远程状态不可用")?;
        state.enabled = false;
        if let Some(flag) = state.dispatch_enabled.take() {
            flag.store(false, std::sync::atomic::Ordering::SeqCst);
        }
        state.phase = "disabled".into();
        if let Some(task) = state.task.take() {
            task.abort();
        }
        Ok(())
    }

    fn start(&self, app: AppHandle) -> Result<(), String> {
        let mut state = self.0.lock().map_err(|_| "远程状态不可用")?;
        if state.enabled {
            return Ok(());
        }
        // Validate local storage before claiming remote is enabled.
        let bridge = Bridge::open(&codex_home(), &storage())?;
        state.dispatch_enabled = Some(bridge.enable_flag());
        let bridge = Arc::new(Mutex::new(bridge));
        state.enabled = true;
        state.phase = "connecting".into();
        let runtime = self.clone();
        state.task = Some(tauri::async_runtime::spawn(async move {
            let mut delay = 1;
            loop {
                let account = ClawkitAccountClient::default();
                if account.active_credentials().is_err() {
                    phase(&app, &runtime, "login-required");
                    break;
                }
                phase(&app, &runtime, "connecting");
                if let Ok(ticket) = account.create_owner_socket_ticket().await {
                    if let Some(url) = ticket["websocketUrl"].as_str() {
                        let result = relay(&app, &runtime, &account, bridge.clone(), url).await;
                        if result.is_ok() {
                            delay = 1;
                        }
                    }
                }
                phase(&app, &runtime, "reconnecting");
                tokio::time::sleep(Duration::from_secs(delay)).await;
                delay = (delay * 2).min(30);
            }
            if let Ok(mut state) = runtime.0.lock() {
                state.enabled = false;
            }
        }));
        Ok(())
    }
}

async fn relay(
    app: &AppHandle,
    state: &DesktopRemoteState,
    account: &ClawkitAccountClient,
    bridge: Arc<Mutex<Bridge>>,
    url: &str,
) -> Result<(), ()> {
    let parsed = url::Url::parse(url).map_err(|_| ())?;
    if parsed.scheme() != "wss" {
        return Err(());
    }
    let config = WebSocketConfig::default()
        .max_message_size(Some(2 * 1024 * 1024))
        .max_frame_size(Some(2 * 1024 * 1024));
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(12),
        tokio_tungstenite::connect_async_with_config(url, Some(config), false),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())?;
    phase(app, state, "waiting");
    let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
    let mut last_received = std::time::Instant::now();
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if last_received.elapsed()>Duration::from_secs(60) {return Err(());}
                if account.active_credentials().is_err() { return Ok(()); }
                socket.send(Message::Ping(Vec::new().into())).await.map_err(|_| ())?;
            }
            message = socket.next() => {
                last_received=std::time::Instant::now();
                match message {
                    Some(Ok(Message::Text(raw))) => {
                        let Ok(envelope) = serde_json::from_str::<Value>(&raw) else {continue;};
                        if envelope["type"] == "relay.peer" && envelope["role"] == "mobile" {
                            phase(app,state,if envelope["online"]==true {"connected"} else {"waiting"});
                        }
                        if envelope["type"] != "relay.data" {continue;}
                        let Some(payload) = envelope["payload"].as_str().filter(|value|value.len()<=32*1024) else {continue;};
                        let Ok(request) = serde_json::from_str::<Value>(payload) else {continue;};
                        let bridge = bridge.clone();
                        let response = tauri::async_runtime::spawn_blocking(move || {
                            bridge.lock().ok().and_then(|bridge|bridge.handle(request))
                        }).await.map_err(|_| ())?;
                        if let Some(response) = response {
                            socket.send(Message::Text(json!({"type":"relay.data","payload":response.to_string()}).to_string().into())).await.map_err(|_| ())?;
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => { socket.send(Message::Pong(payload)).await.map_err(|_| ())?; }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return Err(()),
                    _ => {}
                }
            }
        }
    }
}

#[tauri::command]
pub fn get_desktop_remote_status(state: State<'_, DesktopRemoteState>) -> Result<Value, String> {
    state.status()
}

#[tauri::command]
pub async fn get_desktop_remote_capabilities() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        Bridge::open(&codex_home(), &storage())?.dispatch("desktop/status", &json!({}))
    })
    .await
    .map_err(|_| "检测 Codex 失败".to_string())?
}

#[tauri::command]
pub fn set_desktop_remote_enabled(
    enabled: bool,
    app: AppHandle,
    state: State<'_, DesktopRemoteState>,
) -> Result<Value, String> {
    if enabled {
        ClawkitAccountClient::default().active_credentials()?;
        state.start(app.clone())?;
    } else {
        state.stop()?;
    }
    if let Err(error) =
        crate::config::atomic_write(&preference(), if enabled { b"true" } else { b"false" })
    {
        let _ = state.stop();
        return Err(error.to_string());
    }
    phase(
        &app,
        &state,
        if enabled { "connecting" } else { "disabled" },
    );
    state.status()
}

pub fn restore(app: &AppHandle) {
    let state = app.state::<DesktopRemoteState>();
    if std::fs::read_to_string(preference()).ok().as_deref() == Some("true") {
        let _ = state.start(app.clone());
    }
}

pub fn sign_out(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<DesktopRemoteState>();
    state.stop()?;
    crate::config::atomic_write(&preference(), b"false").map_err(|error| error.to_string())?;
    phase(app, &state, "disabled");
    Ok(())
}

#[tauri::command]
pub fn launch_original_codex() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let app = ["/Applications/ChatGPT.app", "/Applications/Codex.app"]
            .iter()
            .find(|path| std::path::Path::new(path).is_dir())
            .ok_or("请先安装并登录 Codex 桌面端")?;
        std::process::Command::new("/usr/bin/open")
            .arg(app)
            .spawn()
            .map_err(|_| "无法打开 Codex".to_string())?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("请从系统应用菜单打开 Codex；此版本原桌面接续仅验证了 macOS".into())
    }
}
