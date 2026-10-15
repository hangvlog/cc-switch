//! Bounded, process-scoped event replay and single-consumer human requests.
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_EVENTS: usize = 4096;
const MAX_PENDING_BYTES: usize = 512 * 1024;

pub(super) struct Events {
    pub epoch: String,
    seq: u64,
    bytes: usize,
    retained: VecDeque<(u64, Value, usize)>,
    pending: HashMap<String, Value>,
    answered: HashSet<String>,
    answered_order: VecDeque<String>,
    reminders: VecDeque<Value>,
    reminder_store: Option<super::reminders::Reminders>,
    request_reminders: HashMap<String,String>,
}

impl Events {
    pub fn new() -> Self {
        Self { epoch: uuid::Uuid::new_v4().to_string(), seq: 0, bytes: 0,
            retained: VecDeque::new(), pending: HashMap::new(), answered: HashSet::new(),
            answered_order: VecDeque::new(), reminders: VecDeque::new(), reminder_store: None, request_reminders: HashMap::new() }
    }

    pub fn open(home: &std::path::Path) -> Self {
        let mut events = Self::new();
        if let Ok(store) = super::reminders::Reminders::open(home) {
            events.reminders = store.read().unwrap_or_default();
            events.reminder_store = Some(store);
        }
        events
    }

    pub fn observe(&mut self, message: &Value) {
        let Some(method) = message["method"].as_str() else { return };
        if message.get("id").is_some() {
            // Auth refresh, attestation and dynamic tool execution remain desktop-only.
            if !human_request(method) { return; }
            if method == "mcpServer/elicitation/request" && !super::responses::remote_form(message) {
                self.push(json!({"method":"thread/desktopActionRequired","params":{
                    "threadId":message["params"]["threadId"],"reason":"请在电脑完成身份验证"}}));
                return;
            }
            if self.pending.len() >= 128 || self.pending.values().map(|v|v.to_string().len()).sum::<usize>() + message.to_string().len() > MAX_PENDING_BYTES {
                self.push(json!({"method":"thread/desktopActionRequired","params":{
                    "threadId":message["params"]["threadId"],"reason":"待处理请求过多或过大，请在电脑处理"}}));
                return;
            }
            let reminder = self.remind("attention",&message["params"]["threadId"]);
            self.request_reminders.insert(message["id"].to_string(),reminder);
            self.pending.insert(message["id"].to_string(), message.clone());
        } else if method == "serverRequest/resolved" {
            self.resolve(&message["params"]["requestId"]);
            return;
        } else if !["thread/", "turn/", "item/"].iter().any(|prefix| method.starts_with(prefix)) {
            return;
        }
        if method == "turn/completed" {
            self.remind("completed",&message["params"]["threadId"]);
            let ids: Vec<_> = self.pending.values().filter(|r|r["params"]["threadId"]==message["params"]["threadId"]
                && r["params"]["turnId"]==message["params"]["turn"]["id"]).map(|r|r["id"].clone()).collect();
            for id in ids {self.resolve(&id);}
        }
        self.push(message.clone());
    }

    fn push(&mut self, value: Value) {
        self.seq += 1;
        let size = value.to_string().len();
        self.bytes += size;
        self.retained.push_back((self.seq, value, size));
        while self.bytes > MAX_BYTES || self.retained.len() > MAX_EVENTS {
            if let Some((_, _, size)) = self.retained.pop_front() { self.bytes -= size; }
        }
    }

    pub fn read(&self, params: &Value) -> Value {
        let cursor = params["afterSeq"].as_u64().unwrap_or(0);
        let valid_epoch = params["engineInstanceId"].as_str() == Some(self.epoch.as_str());
        let oldest = self.retained.front().map(|v| v.0).unwrap_or(self.seq + 1);
        let reset = !valid_epoch || cursor > self.seq || cursor.saturating_add(1) < oldest;
        let mut size = 0;
        let mut events = Vec::new();
        if !reset {
            for (seq, value, bytes) in self.retained.iter().filter(|v| v.0 > cursor) {
                if size + bytes > 512 * 1024 { break; }
                events.push(json!({"seq":seq,"message":value})); size += bytes;
                if events.len() >= 128 { break; }
            }
        }
        // A single oversized event cannot permanently wedge the cursor.
        let oversized = !reset && cursor < self.seq && events.is_empty();
        let reset = reset || oversized;
        let next = if reset { self.seq } else { events.last().and_then(|v|v["seq"].as_u64()).unwrap_or(cursor) };
        json!({"engineInstanceId":self.epoch,"cursor":next,"headSeq":self.seq,"resetRequired":reset,
            "events":events,"pendingRequests":self.pending.values().collect::<Vec<_>>()})
    }

    fn refresh_reminder_identity(&mut self) {
        if let Some(store)=&mut self.reminder_store {
            match store.refresh_identity() {
                Ok(true)=>self.reminders=store.read().unwrap_or_default(),
                Err(_)=>self.reminders.clear(),
                _=>{},
            }
        }
    }
    fn remind(&mut self,kind:&str,thread:&Value) -> String {
        self.refresh_reminder_identity();
        use sha2::{Digest,Sha256};
        let id=format!("{:x}",Sha256::digest(format!("{}:{}:{kind}:{thread}",self.epoch,self.seq)));
        let event = json!({"id":id,"threadId":thread,"kind":kind,"timestampMs":crate::now_ms()});
        if let Some(store) = &mut self.reminder_store { let _ = store.save(&event); }
        self.reminders.push_back(event);
        while self.reminders.len()>100 {self.reminders.pop_front();}
        id
    }
    pub fn reminders(&mut self)->Value {
        self.refresh_reminder_identity();
        let mut events=self.reminders.clone();
        for event in &mut events {
            if self.request_reminders.values().any(|id|event["id"]==*id) { event["pending"]=json!(true); }
        }
        json!({"events":events,"observedAt":crate::now_ms()})
    }

    pub fn position(&self) -> Value { json!({"engineInstanceId":self.epoch,"cursor":self.seq}) }

    pub fn response(&self, params: &Value) -> Result<Value, String> {
        if params["engineInstanceId"].as_str() != Some(&self.epoch) {
            return Err("引擎已重启，请刷新待处理请求".into());
        }
        let id = &params["serverRequestId"];
        let request = self.pending.get(&id.to_string()).ok_or("请求已处理或已失效，请刷新")?;
        if request["params"]["threadId"] != params["threadId"] {
            return Err("请求不属于当前任务".into());
        }
        let result = super::responses::build(request, params)?;
        Ok(json!({"id":id,"result":result}))
    }

    /// Called under the same lock as the engine write, so desktop/mobile compete atomically.
    pub fn resolve(&mut self, id: &Value) {
        let key = id.to_string();
        if self.pending.remove(&key).is_some() {
            if let Some(reminder) = self.request_reminders.remove(&key) {
                for event in &mut self.reminders { if event["id"]==reminder { event["resolved"]=json!(true); } }
                if let Some(store)=&self.reminder_store { let _=store.resolve(&reminder); }
            }
            self.answered.insert(key.clone()); self.answered_order.push_back(key);
            while self.answered_order.len() > MAX_EVENTS {
                if let Some(key) = self.answered_order.pop_front() { self.answered.remove(&key); }
            }
            self.push(json!({"method":"serverRequest/resolved","params":{"requestId":id}}));
        }
    }

    pub fn already_answered(&self, message: &Value) -> bool {
        message.get("method").is_none() && message.get("id").is_some()
            && self.answered.contains(&message["id"].to_string())
    }
}

fn human_request(method: &str) -> bool {
    matches!(method, "item/commandExecution/requestApproval" | "item/fileChange/requestApproval"
        | "item/permissions/requestApproval" | "item/tool/requestUserInput"
        | "mcpServer/elicitation/request" | "execCommandApproval" | "applyPatchApproval")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reminders_clear_when_authentication_changes_without_restart() {
        let root=tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("auth.json"),"account-a").unwrap();
        let mut events=Events::open(root.path());
        events.observe(&json!({"method":"turn/completed","params":{"threadId":"thread","turn":{"id":"turn"}}}));
        assert_eq!(events.reminders()["events"].as_array().unwrap().len(),1);
        std::fs::write(root.path().join("auth.json"),"account-b").unwrap();
        assert!(events.reminders()["events"].as_array().unwrap().is_empty());
    }
    #[test]
    fn replay_reset_and_auth_exclusion() {
        let mut events = Events::new();
        events.observe(&json!({"id":1,"method":"account/chatgptAuthTokens/refresh","params":{"secret":"hidden"}}));
        assert_eq!(events.seq,0);
        events.observe(&json!({"method":"turn/started","params":{"threadId":"a"}}));
        let snapshot=events.read(&json!({})); assert_eq!(snapshot["resetRequired"],true);
        let replay=events.read(&json!({"engineInstanceId":events.epoch,"afterSeq":0}));
        assert_eq!(replay["events"].as_array().unwrap().len(),1);
        assert_eq!(events.read(&json!({"engineInstanceId":events.epoch,"afterSeq":9}))["resetRequired"],true);
    }
    #[test]
    fn only_one_endpoint_can_answer() {
        let mut events=Events::new();
        events.observe(&json!({"id":7,"method":"item/commandExecution/requestApproval","params":{"threadId":"a"}}));
        let p=json!({"engineInstanceId":events.epoch,"serverRequestId":7,"threadId":"a","decision":"accept"});
        assert_eq!(events.response(&p).unwrap()["result"]["decision"],"accept");
        events.observe(&json!({"id":8,"method":"item/commandExecution/requestApproval","params":{"threadId":"a"}}));
        events.resolve(&json!(7)); assert!(events.response(&p).is_err());
        assert_eq!(events.reminders()["events"][1]["pending"],true);
        assert_ne!(events.reminders()["events"][1]["resolved"],true);
        assert_eq!(events.reminders()["events"][0]["resolved"],true);
        assert!(events.already_answered(&json!({"id":7,"result":{"decision":"accept"}})));
    }
    #[test]
    fn verification_never_leaves_the_desktop_and_completion_expires_questions() {
        let mut events=Events::new();
        for mode in ["url","openai/userVerification"] {
            events.observe(&json!({"id":mode,"method":"mcpServer/elicitation/request","params":{"threadId":"t","mode":mode,"challenge":"PRIVATE_CHALLENGE","url":"PRIVATE_URL"}}));
        }
        let view=events.read(&json!({"engineInstanceId":events.epoch,"afterSeq":0}));
        assert!(!view.to_string().contains("PRIVATE_"));
        assert!(view["pendingRequests"].as_array().unwrap().is_empty());
        events.observe(&json!({"id":9,"method":"item/tool/requestUserInput","params":{"threadId":"t","turnId":"u","questions":[]}}));
        events.observe(&json!({"method":"turn/completed","params":{"threadId":"t","turn":{"id":"u"}}}));
        assert!(events.pending.is_empty());
    }
    #[test]
    fn bounded_history_reports_gaps() {
        let mut events=Events::new();
        for _ in 0..MAX_EVENTS+2 {events.observe(&json!({"method":"item/started","params":{}}));}
        assert_eq!(events.retained.len(),MAX_EVENTS);
        assert_eq!(events.read(&json!({"engineInstanceId":events.epoch,"afterSeq":0}))["resetRequired"],true);
    }
}
