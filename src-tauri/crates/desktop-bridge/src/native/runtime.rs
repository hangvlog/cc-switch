//! Same-engine stdio adapter with bounded replay and atomic human-response routing.
use super::transport::{read_line, LOCAL_LIMIT, RESPONSE_LIMIT};
use super::endpoint::{bind, Stream, fallback};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{BufReader, Write},
    path::{Path, PathBuf},
    process::{ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};
type Pending = Arc<Mutex<HashMap<String, mpsc::SyncSender<Value>>>>;
type Input = Arc<Mutex<Option<ChildStdin>>>;
type DesktopOutput = Arc<Mutex<std::io::Stdout>>;
type EventState = Arc<Mutex<super::events::Events>>;

pub fn maybe_run_adapter() {
    // The GUI and service inherit login-session variables too. They must never
    // be mistaken for a Codex CLI invocation after ordinary-launch opt-in.
    if std::env::args().nth(1).is_none_or(|arg| arg.starts_with("--clawkit-") || arg.starts_with("-psn_") || arg.contains("://") || ["--hidden", "--minimized", "--silent"].contains(&arg.as_str())) {
        return;
    }
    let Some(real) = std::env::var_os("CLAWKIT_CODEX_ENGINE").map(PathBuf::from) else {
        return;
    };
    let current = std::env::current_exe().ok();
    if std::env::var_os("CODEX_CLI_PATH").map(PathBuf::from).and_then(|p|p.canonicalize().ok()) != current.as_ref().and_then(|p|p.canonicalize().ok()) {
        return;
    }
    if !real.is_absolute() || !real.is_file() || Some(&real) == current.as_ref() {
        std::process::exit(2);
    }
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let values: Vec<_> = args.iter().map(|a| a.to_string_lossy()).collect();
    let main = is_main(&values.iter().map(|s| s.as_ref()).collect::<Vec<_>>());
    let mut command = Command::new(&real);
    command
        .args(&args)
        .env_remove("CLAWKIT_CODEX_ENGINE")
        .env("CODEX_CLI_PATH", &real);
    if !main {
        std::process::exit(fallback(&mut command));
    }
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".codex")));
    let Some(home) = home else {
        std::process::exit(2)
    };
    match run(command, &home) {
        Ok(code) => std::process::exit(code),
        Err(_) => {
            eprintln!("ClawKit native runtime unavailable; original configuration was preserved");
            std::process::exit(2);
        }
    }
}

fn is_main(args: &[&str]) -> bool {
    let Some(index) = args.iter().position(|a| *a == "app-server") else {
        return false;
    };
    args.iter().any(|a| *a == "features.code_mode_host=true")
        && !args
            .iter()
            .any(|a| a.starts_with("--listen") || *a == "--stdio")
        && !args[index + 1..].iter().any(|a| {
            [
                "daemon",
                "proxy",
                "generate-ts",
                "generate-json-schema",
                "--help",
                "-h",
            ]
            .contains(a)
        })
        && !args
            .iter()
            .any(|a| a.contains("codex_latex") || a.starts_with("permissions."))
}

fn send(input: &Input, bytes: &[u8]) -> Result<(), String> {
    let mut guard = input.lock().map_err(|_| "引擎输入不可用")?;
    let writer = guard.as_mut().ok_or("桌面已断开")?;
    writer
        .write_all(bytes)
        .and_then(|_| writer.write_all(b"\n"))
        .and_then(|_| writer.flush())
        .map_err(|_| "引擎输入已断开".into())
}


fn run(mut command: Command, home: &Path) -> Result<i32, String> {
    let (listener, _lock, socket) = match bind(home) {
        Ok(bound) => bound,
        Err(_) => {
            // Optional remote creation must never prevent the original desktop from opening.
            eprintln!("ClawKit native endpoint unavailable; starting the original desktop engine");
            return Ok(fallback(&mut command));
        }
    };
    let mut engine = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| e.to_string())?;
    let input = Arc::new(Mutex::new(engine.stdin.take()));
    let output = engine.stdout.take().ok_or("没有引擎输出")?;
    let ready = Arc::new(AtomicBool::new(false));
    let init_id = Arc::new(Mutex::new(None::<Value>));
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let prefix = format!("clawkit-native-{}-", uuid::Uuid::new_v4());
    let events = Arc::new(Mutex::new(super::events::Events::open(home)));
    let desktop_output = Arc::new(Mutex::new(std::io::stdout()));
    {
        let (input, ready, init_id) = (input.clone(), ready.clone(), init_id.clone());
        let events = events.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(std::io::stdin());
            while let Ok(Some(line)) = read_line(&mut reader, 64 * 1024 * 1024) {
                if let Ok(value) = serde_json::from_slice::<Value>(&line) {
                    if value["method"] == "initialize"
                        && ["codex_desktop", "Codex Desktop"]
                            .iter()
                            .any(|name| value["params"]["clientInfo"]["name"] == *name)
                        && value["params"]["capabilities"]["experimentalApi"] == true
                    {
                        *init_id.lock().unwrap() = value.get("id").cloned();
                    }
                }
                let mut state = events.lock().unwrap();
                let message = serde_json::from_slice::<Value>(&line).ok();
                if message.as_ref().is_some_and(|v| state.already_answered(v)) { continue; }
                if send(&input, line.trim_ascii_end()).is_err() { break; }
                if let Some(value) = message.filter(|v| v.get("method").is_none()) {
                    state.resolve(&value["id"]);
                }
            }
            ready.store(false, Ordering::SeqCst);
            // Closing stdin gives the official engine its normal desktop-disconnect semantics.
            input.lock().unwrap().take();
        });
    }
    {
        let (pending, ready, init_id) = (pending.clone(), ready.clone(), init_id.clone());
        let events = events.clone();
        let prefix = prefix.clone();
        let desktop_output = desktop_output.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            while let Ok(Some(line)) = read_line(&mut reader, 64 * 1024 * 1024) {
                if let Ok(mut value) = serde_json::from_slice::<Value>(&line) {
                    events.lock().unwrap().observe(&value);
                    if value.get("method").is_none() {
                        if value.get("id").is_some()
                            && value.get("id") == init_id.lock().unwrap().as_ref()
                            && value.get("result").is_some()
                        {
                            ready.store(true, Ordering::SeqCst);
                        }
                        if let Some(id) = value["id"].as_str().filter(|id| id.starts_with(&prefix))
                        {
                            if let Some(tx) = pending.lock().unwrap().remove(id) {
                                if value["result"].is_object() {
                                    value["result"]["_clawkitStream"] = events.lock().unwrap().position();
                                }
                                let _ = tx.send(value);
                            }
                            continue;
                        }
                    }
                }
                let mut writer = desktop_output.lock().unwrap();
                if writer
                    .write_all(&line)
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
            ready.store(false, Ordering::SeqCst);
            pending.lock().unwrap().clear();
        });
    }
    {
        let (input, ready, pending) = (input.clone(), ready.clone(), pending.clone());
        let events = events.clone();
        std::thread::spawn(move || {
            let count = Arc::new(AtomicUsize::new(0));
            for stream in listener.incoming().flatten() {
                if count.fetch_add(1, Ordering::SeqCst) >= 8 {
                    count.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let (input, ready, pending, count, prefix) = (
                    input.clone(),
                    ready.clone(),
                    pending.clone(),
                    count.clone(),
                    prefix.clone(),
                );
                let events = events.clone();
                let desktop_output = desktop_output.clone();
                std::thread::spawn(move || {
                    let _ = serve(stream, &input, &ready, &pending, &prefix, &events, &desktop_output);
                    count.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
    }
    let status = engine.wait().map_err(|e| e.to_string())?;
    super::endpoint::cleanup(&socket);
    Ok(status.code().unwrap_or(1))
}

fn serve(
    mut stream: Stream,
    input: &Input,
    ready: &AtomicBool,
    pending: &Pending,
    prefix: &str,
    events: &EventState,
    desktop_output: &DesktopOutput,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let raw = read_line(
        &mut BufReader::new(stream.try_clone().map_err(|e| e.to_string())?),
        LOCAL_LIMIT,
    )?
    .ok_or("空请求")?;
    let value: Value = serde_json::from_slice(&raw).map_err(|_| "请求格式无效")?;
    let method = value["method"].as_str().ok_or("缺少方法")?;
    let connected = ready.load(Ordering::SeqCst);
    let response = if !super::allowed(method, &value["params"]) {
        json!({"error":{"code":-32601}})
    } else if method == "clawkit/status" {
        json!({"result":{"ready":connected,"protocolVersion":1,"eventProtocolVersion":1,"engineInstanceId":events.lock().unwrap().epoch}})
    } else if !connected {
        json!({"error":{"code":-32001}})
    } else if method == "clawkit/notifications/read" {
        json!({"result":events.lock().unwrap().reminders()})
    } else if method == "clawkit/events/read" {
        json!({"result":events.lock().unwrap().read(&value["params"])})
    } else if method == "clawkit/request/respond" {
        let mut state = events.lock().unwrap();
        match state.response(&value["params"]) {
            Ok(response) => {
                // Reserve before writing: a partial write is never safe to repeat.
                state.resolve(&response["id"]);
                match send(input, &serde_json::to_vec(&response).map_err(|e| e.to_string())?) {
                    Ok(()) => {
                        let notification=json!({"method":"serverRequest/resolved","params":{"requestId":response["id"]}});
                        if let Ok(mut writer)=desktop_output.lock() {
                            let _=writeln!(writer,"{notification}").and_then(|_|writer.flush());
                        }
                        json!({"result":{"status":"submitted"}})
                    }
                    Err(_) => json!({"error":{"code":-32002,"message":"回答发送结果未知，请刷新请求状态"}}),
                }
            }
            Err(message) => json!({"error":{"code":-32003,"message":message}}),
        }
    } else {
        let id = format!("{prefix}{}", uuid::Uuid::new_v4());
        let (tx, rx) = mpsc::sync_channel(1);
        pending.lock().unwrap().insert(id.clone(), tx);
        let bytes = serde_json::to_vec(&json!({"id":id,"method":method,"params":value["params"]}))
            .map_err(|e| e.to_string())?;
        let result = send(input, &bytes).and_then(|_| {
            rx.recv_timeout(Duration::from_secs(30))
                .map_err(|_| "引擎请求结果未知".into())
        });
        pending.lock().unwrap().remove(&id);
        result.unwrap_or_else(|_| json!({"error":{"code":-32002}}))
    };
    let bytes = serde_json::to_vec(&response).map_err(|e| e.to_string())?;
    if bytes.len() > RESPONSE_LIMIT {
        return Err("响应过大".into());
    }
    stream
        .write_all(&bytes)
        .and_then(|_| stream.write_all(b"\n"))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_default_primary_desktop_server_is_adapted() {
        assert!(is_main(&[
            "-c",
            "features.code_mode_host=true",
            "app-server",
            "-c",
            "plugins.example.enabled=true"
        ]));
        for args in [
            vec!["--version"],
            vec!["app-server"],
            vec![
                "-c",
                "features.code_mode_host=true",
                "app-server",
                "generate-json-schema",
            ],
            vec![
                "-c",
                "features.code_mode_host=true",
                "app-server",
                "--listen=unix://",
            ],
            vec![
                "-c",
                "features.code_mode_host=true",
                "-c",
                "permissions.latex={}",
                "app-server",
            ],
        ] {
            assert!(!is_main(&args));
        }
    }
}
