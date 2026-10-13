//! Native relay lifetime is independent of the settings WebView.
use crate::clawkit_account::ClawkitAccountClient;
#[path = "desktop_remote_events.rs"]
mod events;
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
        let queue_scope = ClawkitAccountClient::default().remote_queue_scope()?;
        let bridge = Bridge::open_with_queue_storage(
            &codex_home(),
            // Preserve durable receipts from prior releases; never lose unknown sends.
            &storage(),
            &storage().join("accounts").join(&queue_scope),
        )?;
        state.dispatch_enabled = Some(bridge.enable_flag());
        let bridge = Arc::new(Mutex::new(bridge));
        state.enabled = true;
        state.phase = "connecting".into();
        let runtime = self.clone();
        state.task = Some(tauri::async_runtime::spawn(async move {
            let queue_bridge = bridge.clone();
            let queue_account = ClawkitAccountClient::default();
            let queue_enabled = state_flag(&queue_bridge);
            let relay_scope = queue_scope.clone();
            struct Disable(std::sync::Arc<std::sync::atomic::AtomicBool>);
            impl Drop for Disable { fn drop(&mut self) { self.0.store(false, std::sync::atomic::Ordering::SeqCst); } }
            let _disable = Disable(queue_enabled.clone());
            let queue_worker = tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    if !queue_enabled.load(std::sync::atomic::Ordering::SeqCst)
                        || queue_account.remote_queue_scope().ok().as_deref()
                            != Some(queue_scope.as_str())
                    {
                        break;
                    }
                    let bridge = queue_bridge.clone();
                    let _ = tauri::async_runtime::spawn_blocking(move || {
                        if let Ok(bridge) = bridge.lock() {
                            let _ = bridge.drain_queue();
                        }
                    })
                    .await;
                }
            });
            let mut delay = 1;
            loop {
                let account = ClawkitAccountClient::default();
                if account.remote_queue_scope().ok().as_deref() != Some(relay_scope.as_str()) {
                    phase(&app, &runtime, "login-required");
                    break;
                }
                phase(&app, &runtime, "connecting");
                match account.create_owner_socket_ticket().await {
                    Ok(ticket) => if let Some(url) = ticket["websocketUrl"].as_str() {
                        if relay(&app, &runtime, &account, bridge.clone(), url, &relay_scope).await.is_ok() { delay = 1; }
                    },
                    Err(error) if error.contains("重新登录") => { phase(&app,&runtime,"login-required");break; },
                    Err(_) => {},
                }
                phase(&app, &runtime, "reconnecting");
                tokio::time::sleep(Duration::from_secs(delay)).await;
                delay = (delay * 2).min(30);
            }
            queue_worker.abort();
            if let Ok(mut state) = runtime.0.lock() {
                state.enabled = false;
            }
        }));
        Ok(())
    }
}

fn state_flag(bridge: &Arc<Mutex<Bridge>>) -> Arc<std::sync::atomic::AtomicBool> {
    bridge.lock().expect("new bridge mutex").enable_flag()
}

async fn relay(
    app: &AppHandle,
    state: &DesktopRemoteState,
    account: &ClawkitAccountClient,
    bridge: Arc<Mutex<Bridge>>,
    url: &str,
    expected_scope: &str,
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
    let (responses, mut outbound) = tokio::sync::mpsc::channel::<Value>(64);
    let event_task = tauri::async_runtime::spawn(events::pump(codex_home(), storage().join("accounts").join(expected_scope), responses.clone()));
    struct Abort(tauri::async_runtime::JoinHandle<()>);
    impl Drop for Abort { fn drop(&mut self) { self.0.abort(); } }
    let _event_task = Abort(event_task);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
    let mut last_received = std::time::Instant::now();
    loop {
        if account.remote_queue_scope().ok().as_deref() != Some(expected_scope) {
            state_flag(&bridge).store(false, std::sync::atomic::Ordering::SeqCst); return Ok(());
        }
        tokio::select! {
            response = outbound.recv() => {
                if let Some(response) = response {
                    for frame in events::frames(response) { socket.send(Message::Text(frame.into())).await.map_err(|_| ())?; }
                }
            }
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
                        let responses = responses.clone();
                        // Bounded dispatch queue; a slow RPC cannot stop heartbeats or events.
                        let permit = responses.try_reserve_owned().map_err(|_| ())?;
                        tauri::async_runtime::spawn_blocking(move || {
                            if let Some(response) = bridge.lock().ok().and_then(|bridge|bridge.handle(request)) {
                                permit.send(response);
                            }
                        });
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
    #[cfg(any(target_os = "macos",windows))]
    {
        let adapter = std::env::current_exe().map_err(|_| "无法定位 ClawKit 运行程序")?;
        let result = clawkit_desktop_bridge::native::launch(&codex_home(), &adapter);
        crate::desktop_remote_log::report(if result.is_ok() {
            "native-launch-requested"
        } else {
            "native-launch-unavailable"
        });
        result
    }
    #[cfg(not(any(target_os = "macos",windows)))]
    {
        Err("请从系统应用菜单打开 Codex；此版本原桌面接续仅验证了 macOS".into())
    }
}
