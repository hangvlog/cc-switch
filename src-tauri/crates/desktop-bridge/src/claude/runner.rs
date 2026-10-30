//! One phone turn = one short-lived CLI process speaking the SDK stream-json
//! protocol, the same one the desktop uses. The process exits when the turn
//! ends, so it never keeps a session from being resumed on the desktop.
use super::{projection, ClaudeEvents};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const MAX_RECEIPTS: usize = 256;
const MAX_STDERR: usize = 4096;

pub struct Launch {
    pub cli: PathBuf,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub args: Vec<String>,
}

pub struct Running {
    pub thread_id: String,
    pub turn_id: String,
    stdin: Mutex<Option<ChildStdin>>,
    child: Mutex<Child>,
    approvals: Mutex<Vec<Value>>,
}

impl Running {
    pub fn approvals(&self) -> Vec<Value> {
        self.approvals.lock().map(|a| a.clone()).unwrap_or_default()
    }
    fn write(&self, value: &Value) -> Result<(), String> {
        let mut stdin = self.stdin.lock().map_err(|_| "执行状态不可用")?;
        let pipe = stdin.as_mut().ok_or("这次执行已结束")?;
        writeln!(pipe, "{value}").and_then(|_| pipe.flush()).map_err(|_| "无法写入 Claude 进程".into())
    }
}

#[derive(Clone, Default)]
pub struct Turns {
    running: Arc<Mutex<HashMap<String, Arc<Running>>>>,
    receipts: Arc<Mutex<Vec<(String, Value)>>>,
}

fn event(thread: &str, turn: &str, kind: &str, extra: Value) -> Value {
    let mut params = json!({"threadId":thread,"turnId":turn,"kind":kind});
    if let (Some(target), Some(source)) = (params.as_object_mut(), extra.as_object()) {
        target.extend(source.clone());
    }
    json!({"method":"claude/event","params":params})
}

fn approval_from(request_id: &str, request: &Value) -> Value {
    let tool = request["tool_name"].as_str().unwrap_or("");
    json!({"requestId":request_id,"toolUseId":request["tool_use_id"],"tool":tool,
        "title":request["display_name"].as_str().or(request["title"].as_str()).unwrap_or(tool),
        "description":projection::clip(request["description"].as_str().unwrap_or(""),1000),
        "input":request["input"],"kind":if tool=="AskUserQuestion" {"question"} else {"approval"}})
}

/// AskUserQuestion takes its answers back through the permission response.
fn answered_input(input: &Value, answers: &Value) -> Value {
    let mut updated = input.clone();
    let mut map = serde_json::Map::new();
    for (index, question) in input["questions"].as_array().into_iter().flatten().enumerate() {
        let Some(text) = question["question"].as_str() else { continue };
        let picked = answers[index.to_string()]["answers"].as_array().or(answers[text]["answers"].as_array());
        let chosen: Vec<&str> = picked.into_iter().flatten().filter_map(Value::as_str).collect();
        if !chosen.is_empty() {
            map.insert(text.into(), json!(chosen.join(", ")));
        }
    }
    updated["answers"] = Value::Object(map);
    updated
}

impl Turns {
    pub fn running(&self, cli_id: &str) -> Option<Arc<Running>> {
        self.running.lock().ok()?.get(cli_id).cloned()
    }

    pub fn receipt(&self, request_id: &str) -> Option<Value> {
        self.receipts.lock().ok()?.iter().find(|(id, _)| id == request_id).map(|(_, v)| v.clone())
    }

    pub fn start(&self, events: &ClaudeEvents, cli_id: &str, thread_id: &str, request_id: &str, text: &str, launch: Launch) -> Result<Value, String> {
        let mut running = self.running.lock().map_err(|_| "执行状态不可用")?;
        if running.contains_key(cli_id) {
            return Err("这个会话正在执行，请等待完成或先停止".into());
        }
        let mut child = Command::new(&launch.cli).args(&launch.args).current_dir(&launch.cwd)
            .env_clear().envs(launch.env.iter().cloned())
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().map_err(|_| "无法启动 Claude Code")?;
        let mut stdin = child.stdin.take().ok_or("无法连接 Claude 进程")?;
        let message = json!({"type":"user","session_id":cli_id,"parent_tool_use_id":null,"message":{"role":"user","content":text}});
        if writeln!(stdin, "{message}").and_then(|_| stdin.flush()).is_err() {
            let _ = child.kill();
            return Err("消息未能交给 Claude Code，未发送".into());
        }
        let stdout = child.stdout.take().ok_or("无法读取 Claude 进程")?;
        let stderr = child.stderr.take();
        let turn_id = uuid::Uuid::new_v4().to_string();
        let turn = Arc::new(Running {
            thread_id: thread_id.into(), turn_id: turn_id.clone(),
            stdin: Mutex::new(Some(stdin)), child: Mutex::new(child), approvals: Mutex::new(Vec::new()),
        });
        running.insert(cli_id.into(), turn.clone());
        drop(running);
        let receipt = json!({"threadId":thread_id,"turnId":turn_id,"accepted":true});
        if let Ok(mut receipts) = self.receipts.lock() {
            if receipts.len() >= MAX_RECEIPTS { receipts.remove(0); }
            receipts.push((request_id.into(), receipt.clone()));
        }
        events.push(event(thread_id, &turn_id, "started", json!({})));
        let errors = Arc::new(Mutex::new(String::new()));
        if let Some(mut pipe) = stderr {
            let errors = errors.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = pipe.read(&mut buf) {
                    if n == 0 { break; }
                    if let Ok(mut text) = errors.lock() {
                        text.push_str(&String::from_utf8_lossy(&buf[..n]));
                        if text.len() > MAX_STDERR { let cut = text.len() - MAX_STDERR; let cut = (cut..text.len()).find(|i| text.is_char_boundary(*i)).unwrap_or(0); text.drain(..cut); }
                    }
                }
            });
        }
        let turns = self.clone();
        let events = events.clone();
        let cli_id = cli_id.to_string();
        std::thread::spawn(move || turns.pump(&events, &cli_id, turn, stdout, errors));
        Ok(receipt)
    }

    fn pump(&self, events: &ClaudeEvents, cli_id: &str, turn: Arc<Running>, stdout: impl Read, errors: Arc<Mutex<String>>) {
        let (thread, turn_id) = (turn.thread_id.clone(), turn.turn_id.clone());
        let mut outcome = json!({"status":"failed","error":"Claude Code 未返回结果"});
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
            match message["type"].as_str() {
                Some("control_request") => {
                    let id = message["request_id"].as_str().unwrap_or("").to_string();
                    let request = &message["request"];
                    if request["subtype"] == "can_use_tool" {
                        let approval = approval_from(&id, request);
                        if let Ok(mut list) = turn.approvals.lock() { list.push(approval.clone()); }
                        events.push(event(&thread, &turn_id, "approval", json!({"request":approval})));
                    } else {
                        let _ = turn.write(&json!({"type":"control_response","response":{"subtype":"error","request_id":id,
                            "error":format!("ClawKit 不支持 {}", request["subtype"].as_str().unwrap_or("该请求"))}}));
                    }
                }
                Some("control_cancel_request") => {
                    let id = message["request_id"].as_str().unwrap_or("");
                    if let Ok(mut list) = turn.approvals.lock() { list.retain(|a| a["requestId"] != id); }
                    events.push(event(&thread, &turn_id, "approvalResolved", json!({"requestId":id})));
                }
                Some(kind @ ("assistant" | "user")) if message["parent_tool_use_id"].is_null() => {
                    let mut items = Vec::new();
                    let role = message["message"]["role"].as_str().unwrap_or(kind);
                    projection::apply_message(&mut items, message["uuid"].as_str().unwrap_or(&turn_id), role, &message["message"]["content"]);
                    events.push(event(&thread, &turn_id, "update", json!({"items":items})));
                }
                Some("result") => {
                    let failed = message["is_error"] == true;
                    outcome = json!({"status":if failed {"failed"} else {"completed"},"durationMs":message["duration_ms"],
                        "costUsd":message["total_cost_usd"],"error":if failed {Some(projection::clip(message["result"].as_str().unwrap_or("执行失败"),500))} else {None}});
                    break;
                }
                _ => {}
            }
        }
        if let Ok(mut stdin) = turn.stdin.lock() { stdin.take(); }
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let done = turn.child.lock().map(|mut c| c.try_wait().ok().flatten().is_some()).unwrap_or(true);
            if done { break; }
            if Instant::now() > deadline { if let Ok(mut c) = turn.child.lock() { let _ = c.kill(); let _ = c.wait(); } break; }
            std::thread::sleep(Duration::from_millis(100));
        }
        if outcome["status"] == "failed" && outcome["durationMs"].is_null() {
            let detail = errors.lock().map(|e| e.trim().to_string()).unwrap_or_default();
            if !detail.is_empty() { outcome["error"] = json!(projection::clip(&detail, 500)); }
        }
        if let Ok(mut running) = self.running.lock() {
            if running.get(cli_id).is_some_and(|r| Arc::ptr_eq(r, &turn)) { running.remove(cli_id); }
        }
        events.push(event(&thread, &turn_id, "completed", outcome));
    }

    pub fn respond(&self, events: &ClaudeEvents, cli_id: &str, request_id: &str, allow: bool, answers: &Value) -> Result<Value, String> {
        let turn = self.running(cli_id).ok_or("这次执行已结束")?;
        let approval = turn.approvals().into_iter().find(|a| a["requestId"] == request_id).ok_or("审批请求已失效")?;
        let decision = if allow {
            let input = if approval["kind"] == "question" { answered_input(&approval["input"], answers) } else { approval["input"].clone() };
            json!({"behavior":"allow","updatedInput":input})
        } else {
            json!({"behavior":"deny","message":"已在 ClawKit 手机端拒绝"})
        };
        turn.write(&json!({"type":"control_response","response":{"subtype":"success","request_id":request_id,"response":decision}}))?;
        if let Ok(mut list) = turn.approvals.lock() { list.retain(|a| a["requestId"] != request_id); }
        events.push(event(&turn.thread_id, &turn.turn_id, "approvalResolved", json!({"requestId":request_id})));
        Ok(json!({"ok":true}))
    }

    pub fn interrupt(&self, cli_id: &str) -> Result<Value, String> {
        let turn = self.running(cli_id).ok_or("没有正在执行的任务")?;
        let sent = turn.write(&json!({"type":"control_request","request_id":uuid::Uuid::new_v4().to_string(),"request":{"subtype":"interrupt"}}));
        let running = self.running.clone();
        let key = cli_id.to_string();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(if sent.is_ok() { 8 } else { 0 }));
            let still = running.lock().ok().and_then(|m| m.get(&key).cloned()).filter(|r| Arc::ptr_eq(r, &turn));
            if let Some(turn) = still { if let Ok(mut c) = turn.child.lock() { let _ = c.kill(); } }
        });
        Ok(json!({"ok":true}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn question_answers_are_keyed_by_question_text() {
        let input = json!({"questions":[{"question":"Which?","options":[{"label":"A"},{"label":"B"}]}]});
        let updated = answered_input(&input, &json!({"0":{"answers":["B"]}}));
        assert_eq!(updated["answers"]["Which?"], "B");
        assert_eq!(updated["questions"], input["questions"]);
    }
}
