//! Start before the official app so its IPC clients share one same-user router.
//! Never replaces a running Codex endpoint or owns an app-server conversation.
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{Read, Write},
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

const MAX_FRAME: usize = 256 * 1024 * 1024; // Match the official IPC frame limit.
pub(super) type Writer = Arc<Mutex<UnixStream>>;
pub(super) struct Client {
    pub(super) writer: Writer,
    pub(super) pid: Option<i32>,
}
pub(super) struct Pending {
    source: String,
    target: Option<String>,
    request: Value,
    deadline: Instant,
    candidates: Vec<String>,
}
#[derive(Default)]
pub(super) struct State {
    pub(super) clients: HashMap<String, Client>,
    pub(super) pending: HashMap<String, Pending>,
}
type Shared = Arc<Mutex<State>>;

fn write(stream: &Writer, value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_FRAME {
        return Err("IPC frame too large".into());
    }
    let mut stream = stream.lock().map_err(|_| "IPC lock unavailable")?;
    stream
        .write_all(&(bytes.len() as u32).to_le_bytes())
        .and_then(|_| stream.write_all(&bytes))
        .map_err(|e| e.to_string())
}
pub(super) fn read(stream: &mut UnixStream) -> Result<Value, String> {
    let mut size = [0; 4];
    stream.read_exact(&mut size).map_err(|e| e.to_string())?;
    let size = u32::from_le_bytes(size) as usize;
    if size == 0 || size > MAX_FRAME {
        return Err("Invalid IPC frame".into());
    }
    let mut data = vec![0; size];
    stream.read_exact(&mut data).map_err(|e| e.to_string())?;
    serde_json::from_slice(&data).map_err(|e| e.to_string())
}
fn failure(id: &str, error: &str) -> Value {
    json!({"type":"response","requestId":id,"resultType":"error","error":error})
}
fn status(id: &str, kind: &str, value: &str) -> Value {
    json!({"type":"broadcast","method":"client-status-changed","sourceClientId":id,"version":0,
        "params":{"clientId":id,"clientType":kind,"status":value}})
}

/// Return deliveries under the state lock; socket I/O always happens outside it.
pub(super) fn route(state: &mut State, source: &str, mut value: Value) -> Vec<(Writer, Value)> {
    let mut output = Vec::new();
    let Some(caller) = state.clients.get(source) else {
        return output;
    };
    match value["type"].as_str() {
        Some("broadcast") => {
            value["sourceClientId"] = json!(source);
            for (id, client) in &state.clients {
                if id != source
                    && value["targetClientIds"]
                        .as_array()
                        .is_none_or(|ids| ids.iter().any(|v| v == id))
                {
                    output.push((client.writer.clone(), value.clone()));
                }
            }
        }
        Some("request") => {
            let Some(id) = value["requestId"].as_str().map(str::to_owned) else {
                return output;
            };
            if value["method"] == "initialize" {
                output.push((caller.writer.clone(), json!({"type":"response","requestId":id,"method":"initialize",
                    "resultType":"success","handledByClientId":source,"result":{"clientId":source}})));
                return output;
            }
            if state.pending.len() >= 256 {
                output.push((caller.writer.clone(), failure(&id, "router-busy")));
                return output;
            }
            let route_id = uuid::Uuid::new_v4().to_string();
            value["sourceClientId"] = json!(source);
            let candidates: Vec<String> = state
                .clients
                .keys()
                .filter(|id| {
                    id.as_str() != source
                        && value["targetClientId"]
                            .as_str()
                            .is_none_or(|target| target == id.as_str())
                })
                .cloned()
                .collect();
            for target in &candidates {
                output.push((
                    state.clients[target].writer.clone(),
                    json!({"type":"client-discovery-request","requestId":route_id,"request":value}),
                ));
            }
            if candidates.is_empty() {
                output.push((caller.writer.clone(), failure(&id, "no-client-found")));
            } else {
                state.pending.insert(
                    route_id,
                    Pending {
                        source: source.into(),
                        target: None,
                        request: value,
                        deadline: Instant::now() + Duration::from_secs(10),
                        candidates,
                    },
                );
            }
        }
        Some("client-discovery-response") => {
            let Some(id) = value["requestId"].as_str() else {
                return output;
            };
            if let Some(pending) = state.pending.get_mut(id) {
                if pending.target.is_some() || !pending.candidates.iter().any(|id| id == source) {
                    return output;
                }
                pending.candidates.retain(|id| id != source);
                if value["response"]["canHandle"] == true {
                    pending.target = Some(source.into());
                    pending.deadline = Instant::now()
                        + Duration::from_millis(
                            pending.request["timeoutMs"]
                                .as_u64()
                                .unwrap_or(30_000)
                                .clamp(1000, 120_000),
                        );
                    let mut request = pending.request.clone();
                    request["requestId"] = json!(id);
                    output.push((caller.writer.clone(), request));
                } else if pending.candidates.is_empty() {
                    let pending = state.pending.remove(id).unwrap();
                    if let Some(client) = state.clients.get(&pending.source) {
                        output.push((
                            client.writer.clone(),
                            failure(
                                pending.request["requestId"].as_str().unwrap_or(""),
                                "no-client-found",
                            ),
                        ));
                    }
                }
            }
        }
        Some("response") => {
            let Some(id) = value["requestId"].as_str().map(str::to_owned) else {
                return output;
            };
            if state
                .pending
                .get(&id)
                .is_some_and(|p| p.target.as_deref() == Some(source))
            {
                let pending = state.pending.remove(&id).unwrap();
                if let Some(client) = state.clients.get(&pending.source) {
                    value["requestId"] = pending.request["requestId"].clone();
                    value["handledByClientId"] = json!(source);
                    output.push((client.writer.clone(), value));
                }
            }
        }
        _ => {}
    }
    output
}
pub(super) fn deliver(items: Vec<(Writer, Value)>) {
    for (writer, value) in items {
        if write(&writer, &value).is_err() {
            if let Ok(stream) = writer.lock() {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        }
    }
}
fn disconnect(state: &mut State, id: &str, kind: &str) -> Vec<(Writer, Value)> {
    let mut output = route(state, id, status(id, kind, "disconnected"));
    state.clients.remove(id);
    let affected: Vec<_> = state
        .pending
        .iter_mut()
        .filter_map(|(key, p)| {
            p.candidates.retain(|candidate| candidate != id);
            (p.source == id
                || p.target.as_deref() == Some(id)
                || (p.target.is_none() && p.candidates.is_empty()))
            .then(|| key.clone())
        })
        .collect();
    for key in affected {
        let pending = state.pending.remove(&key).unwrap();
        if let Some(client) = state.clients.get(&pending.source) {
            output.push((
                client.writer.clone(),
                failure(
                    pending.request["requestId"].as_str().unwrap_or(""),
                    "client-disconnected",
                ),
            ));
        }
    }
    output
}

#[cfg(target_os="macos")]
fn peer_pid(stream: &UnixStream) -> Option<i32> {
    use std::os::fd::AsRawFd;
    let mut pid: i32 = 0;
    let mut len = std::mem::size_of::<i32>() as libc::socklen_t;
    let ok = unsafe { libc::getsockopt(stream.as_raw_fd(),0,libc::LOCAL_PEERPID,
        &mut pid as *mut _ as *mut libc::c_void,&mut len) };
    (ok == 0 && pid > 1).then_some(pid)
}
#[cfg(not(target_os="macos"))]
fn peer_pid(_stream: &UnixStream) -> Option<i32> { None }

fn serve(mut stream: UnixStream, state: Shared) -> Result<(), String> {
    stream.set_nonblocking(false).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| e.to_string())?;
    let pid = peer_pid(&stream);
    let initial = read(&mut stream)?;
    if initial["type"] != "request" || initial["method"] != "initialize" {
        return Err("Initialize required".into());
    }
    let id = uuid::Uuid::new_v4().to_string();
    let kind = initial["params"]["clientType"]
        .as_str()
        .unwrap_or("unknown")
        .chars()
        .take(64)
        .collect::<String>();
    let writer = Arc::new(Mutex::new(stream.try_clone().map_err(|e| e.to_string())?));
    let output = {
        let mut state = state.lock().unwrap();
        if state.clients.len() >= 128 {
            return Err("Too many IPC clients".into());
        }
        state.clients.insert(id.clone(), Client { writer, pid });
        let mut output = route(&mut state, &id, initial);
        output.extend(route(&mut state, &id, status(&id, &kind, "connected")));
        output
    };
    deliver(output);
    // Idle sockets consume no CPU. Frame limits bound memory; disconnect terminates the worker.
    stream.set_read_timeout(None).map_err(|e| e.to_string())?;
    loop {
        match read(&mut stream) {
            Ok(value) => {
                let output = route(&mut state.lock().unwrap(), &id, value);
                deliver(output);
            }
            Err(_) => break,
        }
    }
    let output = {
        let mut state = state.lock().unwrap();
        disconnect(&mut state, &id, &kind)
    };
    deliver(output);
    Ok(())
}

pub fn run(home: &Path, keeper: Option<i32>) -> Result<(), String> {
    let directory = home.join("ipc");
    if !directory.exists() {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|e| e.to_string())?;
    }
    let meta = std::fs::symlink_metadata(&directory).map_err(|e| e.to_string())?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
        return Err("IPC directory permissions invalid".into());
    }
    let path = directory.join("ipc.sock");
    // bind fails for existing sockets: never unlink or take over an official live endpoint.
    let listener = UnixListener::bind(&path).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let state = Arc::new(Mutex::new(State::default()));
    let marker = directory.join("clawkit-router.json");
    std::fs::write(&marker,json!({"pid":std::process::id(),"inode":std::fs::symlink_metadata(&path).map_err(|e|e.to_string())?.ino(),"version":1}).to_string()).map_err(|e|e.to_string())?;
    let connections = Arc::new(AtomicUsize::new(0));
    let inode = std::fs::symlink_metadata(&path)
        .map_err(|e| e.to_string())?
        .ino();
    let mut last_connected = Instant::now();
    let mut published_pids = Vec::<i32>::new();
    loop {
        while let Ok((stream, _)) = listener.accept() {
            if connections.load(Ordering::Relaxed) >= 128 {
                continue;
            }
            connections.fetch_add(1, Ordering::Relaxed);
            let state = state.clone();
            let connections = connections.clone();
            std::thread::spawn(move || {
                let _ = serve(stream, state);
                connections.fetch_sub(1, Ordering::Relaxed);
            });
        }
        let output = {
            let mut state = state.lock().unwrap();
            if !state.clients.is_empty() {
                last_connected = Instant::now();
            }
            let expired: Vec<_> = state
                .pending
                .iter()
                .filter(|(_, p)| p.deadline <= Instant::now())
                .map(|(id, _)| id.clone())
                .collect();
            expired
                .into_iter()
                .filter_map(|id| {
                    let p = state.pending.remove(&id)?;
                    let c = state.clients.get(&p.source)?;
                    Some((
                        c.writer.clone(),
                        failure(
                            p.request["requestId"].as_str().unwrap_or(""),
                            "no-client-found",
                        ),
                    ))
                })
                .collect()
        };
        deliver(output);
        let mut pids: Vec<i32> = state.lock().unwrap().clients.values().filter_map(|c|c.pid).collect();
        pids.sort_unstable(); pids.dedup();
        if pids != published_pids && std::fs::symlink_metadata(&path).is_ok_and(|m|m.ino()==inode) {
            let tmp=marker.with_extension(format!("{}.tmp",std::process::id()));
            let body=json!({"pid":std::process::id(),"inode":inode,"version":1,"peerPids":pids});
            if std::fs::write(&tmp,body.to_string()).is_ok() { let _=std::fs::rename(tmp,&marker); }
            published_pids=pids;
        }
        if last_connected.elapsed() > Duration::from_secs(30)
            && !keeper.is_some_and(|pid| unsafe { libc::kill(pid,0) } == 0) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.ino() == inode) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(marker);
    }
    Ok(())
}

