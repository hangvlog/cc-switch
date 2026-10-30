//! Where the Claude desktop app keeps its Code sessions, and which of them a
//! CLI process is currently writing. Everything here is read from disk.
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

const MAX_REGISTRY: usize = 200;

#[derive(Clone, Debug)]
pub struct Paths {
    pub claude_home: PathBuf,
    pub app_data: Option<PathBuf>,
    pub registry: PathBuf,
    pub cli_override: Option<PathBuf>,
}

impl Paths {
    pub fn detect(storage: &Path) -> Self {
        let home = home_dir();
        let claude_home = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".claude"));
        let app_data = app_candidates(&home)
            .into_iter()
            .find(|p| p.join("claude-code-sessions").is_dir());
        Self {
            claude_home,
            app_data,
            registry: storage.join("claude-sessions.json"),
            cli_override: std::env::var_os("CLAWKIT_CLAUDE_CLI").map(PathBuf::from),
        }
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn app_candidates(home: &Path) -> Vec<PathBuf> {
    let base = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| home.join("AppData/Roaming"))
    } else {
        home.join(".config")
    };
    vec![base.join("Claude-3p"), base.join("Claude")]
}

#[derive(Clone, Debug)]
pub struct Live {
    pub pid: i64,
    pub status: String,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub id: String,
    pub cli_id: String,
    pub cwd: String,
    pub title: String,
    pub created_at: u64,
    pub activity_at: u64,
    pub archived: bool,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub cli_version: Option<String>,
    pub transcript: Option<PathBuf>,
    pub transcript_at: u64,
    pub live: Option<Live>,
}

impl Session {
    /// Desktop metadata is not touched by phone turns; the transcript is.
    pub fn updated_at(&self) -> u64 {
        self.activity_at.max(self.transcript_at)
    }
}

pub fn is_uuid(value: &str) -> bool {
    value.len() == 36 && uuid::Uuid::parse_str(value).is_ok()
}

pub fn encode_project(cwd: &str) -> String {
    cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

fn modified_ms(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn find_transcript(paths: &Paths, cli_id: &str, cwd: &str) -> Option<PathBuf> {
    if !is_uuid(cli_id) {
        return None;
    }
    let projects = paths.claude_home.join("projects");
    let name = format!("{cli_id}.jsonl");
    let mut variants = vec![cwd.to_string()];
    if let Ok(real) = fs::canonicalize(cwd) {
        variants.push(real.to_string_lossy().into_owned());
    }
    for variant in variants.iter().filter(|v| !v.is_empty()) {
        let path = projects.join(encode_project(variant)).join(&name);
        if path.is_file() {
            return Some(path);
        }
    }
    let mut found = fs::read_dir(&projects).ok()?.flatten().map(|e| e.path().join(&name)).filter(|p| p.is_file());
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

#[cfg(unix)]
fn pid_alive(pid: i64) -> bool {
    if pid <= 1 || pid > i32::MAX as i64 {
        return false;
    }
    let alive = unsafe { libc::kill(pid as i32, 0) == 0 };
    alive || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
#[cfg(not(unix))]
fn pid_alive(pid: i64) -> bool {
    pid > 0
}

/// CLI session id -> process currently attached to it (desktop or terminal).
pub fn live_processes(paths: &Paths) -> HashMap<String, Live> {
    let mut live = HashMap::new();
    let Ok(entries) = fs::read_dir(paths.claude_home.join("sessions")) else { return live };
    for path in entries.flatten().map(|e| e.path()) {
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(record) = fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { continue };
        let (Some(pid), Some(id)) = (record["pid"].as_i64(), record["sessionId"].as_str()) else { continue };
        if pid_alive(pid) {
            live.insert(id.to_string(), Live { pid, status: record["status"].as_str().unwrap_or("unknown").into() });
        }
    }
    live
}

fn desktop_records(paths: &Paths) -> Vec<Value> {
    let Some(base) = paths.app_data.as_ref().map(|p| p.join("claude-code-sessions")) else { return vec![] };
    let mut records = Vec::new();
    for account in fs::read_dir(base).into_iter().flatten().flatten() {
        for org in fs::read_dir(account.path()).into_iter().flatten().flatten() {
            for file in fs::read_dir(org.path()).into_iter().flatten().flatten() {
                let name = file.file_name().to_string_lossy().into_owned();
                if !(name.starts_with("local_") && name.ends_with(".json")) {
                    continue;
                }
                if let Some(v) = fs::read(file.path()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
                    records.push(v);
                }
            }
        }
    }
    records
}

fn session_from(paths: &Paths, raw: &Value, live: &HashMap<String, Live>) -> Option<Session> {
    let cli_id = raw["cliSessionId"].as_str().filter(|v| is_uuid(v))?.to_string();
    let id = raw["sessionId"].as_str()?.to_string();
    let cwd = raw["cwd"].as_str().or(raw["originCwd"].as_str()).unwrap_or("").to_string();
    let transcript = find_transcript(paths, &cli_id, &cwd);
    Some(Session {
        transcript_at: transcript.as_deref().map(modified_ms).unwrap_or(0),
        transcript,
        live: live.get(&cli_id).cloned(),
        title: raw["title"].as_str().unwrap_or("").into(),
        created_at: raw["createdAt"].as_u64().unwrap_or(0),
        activity_at: raw["lastActivityAt"].as_u64().unwrap_or(0),
        archived: raw["isArchived"] == true,
        model: raw["model"].as_str().map(Into::into),
        permission_mode: raw["permissionMode"].as_str().map(Into::into),
        cli_version: raw["cliBinaryPin"]["version"].as_str().map(Into::into),
        id,
        cli_id,
        cwd,
    })
}

pub fn read_registry(paths: &Paths) -> Vec<Value> {
    fs::read(&paths.registry)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

/// Sessions started from the phone are not in the desktop index until imported there.
pub fn remember(paths: &Paths, cli_id: &str, cwd: &str, title: &str) -> Result<(), String> {
    let mut entries = read_registry(paths);
    entries.retain(|e| e["cliSessionId"] != cli_id);
    entries.push(json!({"cliSessionId":cli_id,"cwd":cwd,"title":title,"createdAt":crate::now_ms()}));
    let start = entries.len().saturating_sub(MAX_REGISTRY);
    if let Some(parent) = paths.registry.parent() {
        fs::create_dir_all(parent).map_err(|_| "无法保存 Claude 会话记录")?;
    }
    let tmp = paths.registry.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&tmp, Value::Array(entries[start..].to_vec()).to_string())
        .and_then(|_| fs::rename(&tmp, &paths.registry))
        .map_err(|_| "无法保存 Claude 会话记录".into())
}

pub fn sessions(paths: &Paths, include_archived: bool) -> Vec<Session> {
    let live = live_processes(paths);
    let mut list: Vec<Session> = desktop_records(paths).iter().filter_map(|r| session_from(paths, r, &live)).collect();
    let known: std::collections::HashSet<String> = list.iter().map(|s| s.cli_id.clone()).collect();
    for entry in read_registry(paths) {
        let Some(cli_id) = entry["cliSessionId"].as_str().filter(|id| is_uuid(id) && !known.contains(*id)) else { continue };
        let raw = json!({"sessionId":format!("cli:{cli_id}"),"cliSessionId":cli_id,"cwd":entry["cwd"],
            "title":entry["title"],"createdAt":entry["createdAt"],"lastActivityAt":entry["createdAt"]});
        // A new session has no transcript until the CLI writes its first record.
        let fresh = crate::now_ms().saturating_sub(entry["createdAt"].as_u64().unwrap_or(0)) < 10 * 60 * 1000;
        if let Some(session) = session_from(paths, &raw, &live).filter(|s| s.transcript.is_some() || fresh) {
            list.push(session);
        }
    }
    if !include_archived {
        list.retain(|s| !s.archived);
    }
    list.sort_by(|a, b| b.updated_at().cmp(&a.updated_at()).then_with(|| a.id.cmp(&b.id)));
    list
}

pub fn resolve(paths: &Paths, id: &str) -> Result<Session, String> {
    let valid = id.strip_prefix("local_").or_else(|| id.strip_prefix("cli:")).is_some_and(is_uuid);
    if !valid {
        return Err("Claude 会话 ID 无效".into());
    }
    sessions(paths, true).into_iter().find(|s| s.id == id).ok_or_else(|| "电脑上找不到这个 Claude 会话".into())
}
