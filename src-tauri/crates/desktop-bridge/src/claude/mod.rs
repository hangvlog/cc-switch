//! Claude desktop (Code tab) sessions on this computer, exposed to the phone.
//! Lists and transcripts come from the desktop's own files; a phone message
//! runs one CLI turn on the gateway the desktop is configured with. A session
//! whose CLI process is alive (open in the desktop) is read-only: two writers
//! silently fork the conversation.
mod gateway;
mod index;
mod projection;
mod runner;
#[cfg(all(test, unix))]
mod tests;

pub use index::Paths;
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const MAX_EVENTS: usize = 512;
const MAX_MESSAGE: usize = 20_000;

#[derive(Clone, Default)]
pub struct ClaudeEvents(Arc<Mutex<VecDeque<Value>>>);

impl ClaudeEvents {
    pub fn push(&self, value: Value) {
        if let Ok(mut queue) = self.0.lock() {
            if queue.len() >= MAX_EVENTS {
                queue.pop_front();
            }
            queue.push_back(value);
        }
    }
    pub fn drain(&self) -> Vec<Value> {
        self.0.lock().map(|mut q| q.drain(..).collect()).unwrap_or_default()
    }
}

pub struct Claude {
    paths: Paths,
    events: ClaudeEvents,
    turns: runner::Turns,
}

fn request_id(params: &Value) -> Result<&str, String> {
    params["requestId"].as_str().filter(|id| index::is_uuid(id)).ok_or_else(|| "发送请求编号必须为 UUID".into())
}

fn message_text(params: &Value) -> Result<&str, String> {
    let text = params["text"].as_str().map(str::trim).unwrap_or("");
    if text.is_empty() {
        return Err("请输入消息".into());
    }
    if text.len() > MAX_MESSAGE {
        return Err("消息过长".into());
    }
    Ok(text)
}

fn permission_mode(mode: Option<&str>) -> &'static str {
    match mode {
        Some("default") => "default",
        Some("plan") => "plan",
        // Bypass and classifier modes are not granted to remote turns.
        _ => "acceptEdits",
    }
}

impl Claude {
    pub fn open(storage: &Path) -> Self {
        // Resolving the login-shell PATH takes seconds; keep it off the RPC path.
        std::thread::spawn(gateway::shell_path);
        Self::with_paths(Paths::detect(storage))
    }

    pub fn with_paths(paths: Paths) -> Self {
        Self { paths, events: ClaudeEvents::default(), turns: runner::Turns::default() }
    }

    pub fn events(&self) -> ClaudeEvents {
        self.events.clone()
    }

    fn readiness(&self) -> Result<Vec<String>, String> {
        if self.paths.app_data.is_none() {
            return Err("这台电脑未安装 Claude 桌面端".into());
        }
        let models = gateway::gateway(&self.paths)?.models;
        gateway::cli(&self.paths, None)?;
        Ok(models)
    }

    pub fn capability(&self) -> Value {
        match self.readiness() {
            Ok(models) => json!({"available":true,"protocolVersion":1,"models":models}),
            Err(reason) => json!({"available":false,"protocolVersion":1,"reason":reason}),
        }
    }

    pub fn dispatch(&self, method: &str, params: &Value) -> Result<Value, String> {
        match method {
            "claude/status" => Ok(self.capability()),
            "claude/thread/list" => self.list(params),
            "claude/thread/read" => self.read(params),
            "claude/thread/create" => self.create(params),
            "claude/turn/start" => self.start(params),
            "claude/turn/interrupt" => {
                let session = index::resolve(&self.paths, params["threadId"].as_str().unwrap_or(""))?;
                self.turns.interrupt(&session.cli_id)
            }
            "claude/request/respond" => {
                let session = index::resolve(&self.paths, params["threadId"].as_str().unwrap_or(""))?;
                let id = params["requestId"].as_str().filter(|id| !id.is_empty() && id.len() < 200).ok_or("缺少审批编号")?;
                let allow = matches!(params["decision"].as_str(), Some("accept" | "acceptForSession"));
                self.turns.respond(&self.events, &session.cli_id, id, allow, &params["answers"])
            }
            _ => Err("此 Claude 操作不受支持".into()),
        }
    }

    fn summary(&self, session: &index::Session) -> Value {
        let running = self.turns.running(&session.cli_id);
        let (can_send, reason) = if running.is_some() {
            (false, Some("正在执行，可等待完成或停止"))
        } else if session.live.is_some() {
            (false, Some("此会话正在电脑上打开，暂不能从手机发送；电脑闲置几分钟后会自动释放"))
        } else if session.transcript.is_none() {
            (false, Some("电脑上没有这个会话的记录"))
        } else {
            (true, None)
        };
        json!({
            "id":session.id,"engine":"claude","cliSessionId":session.cli_id,
            "name":(!session.title.is_empty()).then_some(&session.title),"preview":"","cwd":session.cwd,
            "createdAt":session.created_at / 1000,"updatedAt":session.updated_at() / 1000,"updatedAtMs":session.updated_at(),
            "model":session.model,"archived":session.archived,"turns":[],
            "status":{"type":if running.is_some() {"active"} else {"idle"}},
            "desktop":{
                "source":"claude-desktop","observedAt":crate::now_ms(),
                "status":if running.is_some() {"running"} else if session.live.is_some() {"desktop-open"} else {"idle"},
                "live":session.live.as_ref().map(|l| json!({"pid":l.pid,"status":l.status})),
                "canSend":can_send,"reason":reason,"canInterrupt":running.is_some(),
                "activeTurnId":running.as_ref().map(|r| r.turn_id.clone()),
                "approvals":running.as_ref().map(|r| r.approvals()).unwrap_or_default(),
                "model":session.model,
            }
        })
    }

    fn list(&self, params: &Value) -> Result<Value, String> {
        let limit = params["limit"].as_u64().unwrap_or(50).clamp(1, 200) as usize;
        let sessions = index::sessions(&self.paths, params["includeArchived"] == true);
        let projects: Vec<String> = {
            let mut cwds: Vec<String> = sessions.iter().map(|s| s.cwd.clone()).filter(|c| !c.is_empty()).collect();
            cwds.dedup();
            let mut seen = std::collections::HashSet::new();
            cwds.retain(|c| seen.insert(c.clone()));
            cwds
        };
        Ok(json!({
            "data":sessions.iter().take(limit).map(|s| self.summary(s)).collect::<Vec<_>>(),
            "total":sessions.len(),"projects":projects,"capability":self.capability(),
        }))
    }

    fn read(&self, params: &Value) -> Result<Value, String> {
        let session = index::resolve(&self.paths, params["threadId"].as_str().unwrap_or(""))?;
        let path = session.transcript.as_ref().ok_or("电脑上没有这个会话的记录")?;
        let limit = params["limit"].as_u64().unwrap_or(20).clamp(1, 100) as usize;
        let running = self.turns.running(&session.cli_id).map(|r| r.turn_id.clone());
        let projection = projection::project(&projection::read_records(path)?, limit, running.as_deref());
        let mut thread = self.summary(&session);
        thread["preview"] = json!(projection.preview);
        thread["turns"] = Value::Array(projection.turns);
        thread["totalTurns"] = json!(projection.total_turns);
        Ok(json!({"thread":thread}))
    }

    fn launch(&self, cwd: &str, session_args: [&str; 2], model: Option<&str>, mode: Option<&str>, pin: Option<&str>) -> Result<runner::Launch, String> {
        let gateway = gateway::gateway(&self.paths)?;
        let cli = gateway::cli(&self.paths, pin)?;
        let mut args: Vec<String> = ["-p", "--verbose", "--input-format", "stream-json", "--output-format", "stream-json",
            "--permission-prompt-tool", "stdio"].iter().map(|s| s.to_string()).collect();
        args.extend(session_args.iter().map(|s| s.to_string()));
        if let Some(model) = model.filter(|m| !m.is_empty() && m.len() < 200) {
            args.extend(["--model".into(), model.into()]);
        }
        args.extend(["--permission-mode".into(), permission_mode(mode).into()]);
        Ok(runner::Launch { cli, cwd: PathBuf::from(cwd), env: gateway::child_env(&gateway), args })
    }

    fn start(&self, params: &Value) -> Result<Value, String> {
        let id = request_id(params)?;
        if let Some(receipt) = self.turns.receipt(id) {
            return Ok(receipt);
        }
        let text = message_text(params)?;
        let session = index::resolve(&self.paths, params["threadId"].as_str().unwrap_or(""))?;
        if session.live.is_some() {
            return Err("此会话正在电脑上打开，暂不能从手机发送；电脑闲置几分钟后会自动释放".into());
        }
        if session.transcript.is_none() {
            return Err("电脑上没有这个会话的记录".into());
        }
        let launch = self.launch(&session.cwd, ["--resume", &session.cli_id], session.model.as_deref(),
            session.permission_mode.as_deref(), session.cli_version.as_deref())?;
        self.turns.start(&self.events, &session.cli_id, &session.id, id, text, launch)
    }

    fn create(&self, params: &Value) -> Result<Value, String> {
        let id = request_id(params)?;
        if let Some(receipt) = self.turns.receipt(id) {
            return Ok(receipt);
        }
        let text = message_text(params)?;
        let cwd = params["cwd"].as_str().unwrap_or("");
        let sessions = index::sessions(&self.paths, true);
        if !sessions.iter().any(|s| s.cwd == cwd) || !Path::new(cwd).is_dir() {
            return Err("只能在电脑上已有 Claude 会话的项目目录里新建".into());
        }
        let model = params["model"].as_str().map(String::from)
            .or_else(|| sessions.iter().find(|s| s.cwd == cwd).and_then(|s| s.model.clone()));
        let cli_id = uuid::Uuid::new_v4().to_string();
        let launch = self.launch(cwd, ["--session-id", &cli_id], model.as_deref(), Some("acceptEdits"), None)?;
        index::remember(&self.paths, &cli_id, cwd, &projection::clip(text, 60))?;
        self.turns.start(&self.events, &cli_id, &format!("cli:{cli_id}"), id, text, launch)
    }
}
