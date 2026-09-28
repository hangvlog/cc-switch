//! Versioned local desktop IPC. Never starts an app-server or becomes a task owner.
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    path::Path,
    time::{Duration, Instant},
};
use uuid::Uuid;

const MAX_FRAME: usize = 32 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(unix)]
type Stream = std::os::unix::net::UnixStream;
#[cfg(not(unix))]
type Stream = std::fs::File;

pub struct IpcClient {
    stream: Stream,
    client_id: String,
}

pub struct Snapshot {
    pub owner: String,
    pub revision: u64,
    pub state: Value,
}

impl IpcClient {
    #[cfg(unix)]
    pub fn connect(home: &Path) -> Result<Self, String> {
        #[cfg(unix)]
        let stream = {
            use std::os::unix::fs::{FileTypeExt, MetadataExt};
            let path = home.join("ipc/ipc.sock");
            let metadata =
                std::fs::symlink_metadata(&path).map_err(|_| "Codex 桌面未运行或 IPC 不可用")?;
            // Same-user IPC only. Never create, chmod, or replace Codex's socket.
            if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } {
                return Err("Codex IPC 不属于当前用户".into());
            }
            let stream = Stream::connect(path).map_err(|_| "无法连接 Codex 桌面")?;
            stream
                .set_read_timeout(Some(TIMEOUT))
                .map_err(|e| e.to_string())?;
            stream
                .set_write_timeout(Some(TIMEOUT))
                .map_err(|e| e.to_string())?;
            stream
        };
        let mut client = Self {
            stream,
            client_id: "initializing-client".into(),
        };
        let initialized = client.request(
            "initialize",
            json!({"clientType":"clawkit-remote"}),
            0,
            None,
        )?;
        client.client_id = initialized["result"]["clientId"]
            .as_str()
            .ok_or("Codex IPC 初始化响应不兼容")?
            .to_owned();
        Ok(client)
    }

    #[cfg(not(unix))]
    pub fn connect(_home: &Path) -> Result<Self, String> {
        Err("当前系统的原桌面 IPC 尚未完成适配，不能发送任务".into())
    }

    fn send(&mut self, value: &Value) -> Result<(), String> {
        let body = serde_json::to_vec(value).map_err(|e| e.to_string())?;
        if body.len() > MAX_FRAME {
            return Err("IPC 消息超过上限".into());
        }
        self.stream
            .write_all(&(body.len() as u32).to_le_bytes())
            .and_then(|_| self.stream.write_all(&body))
            .map_err(|_| "IPC 写入失败，需核对发送回执".into())
    }

    fn receive(&mut self) -> Result<Value, String> {
        let mut size = [0; 4];
        self.stream
            .read_exact(&mut size)
            .map_err(|_| "Codex IPC 响应超时或连接中断")?;
        let size = u32::from_le_bytes(size) as usize;
        if size == 0 || size > MAX_FRAME {
            return Err("Codex IPC 帧长度不兼容".into());
        }
        let mut body = vec![0; size];
        self.stream
            .read_exact(&mut body)
            .map_err(|_| "Codex IPC 帧未接收完整")?;
        serde_json::from_slice(&body).map_err(|_| "Codex IPC 响应格式不兼容".into())
    }

    fn decline_discovery(&mut self, event: &Value) -> Result<(), String> {
        if event["type"] == "client-discovery-request" {
            self.send(
                &json!({"type":"client-discovery-response", "requestId":event["requestId"],
                "response":{"canHandle":false}}),
            )?;
        }
        Ok(())
    }

    pub fn request(
        &mut self,
        method: &str,
        params: Value,
        version: u32,
        target: Option<&str>,
    ) -> Result<Value, String> {
        let id = Uuid::new_v4().to_string();
        let mut request = json!({"type":"request", "requestId":id, "sourceClientId":self.client_id,
            "version":version, "method":method, "params":params, "timeoutMs":4000});
        if let Some(target) = target {
            request["targetClientId"] = json!(target);
        }
        self.send(&request)?;
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            let response = self.receive()?;
            self.decline_discovery(&response)?;
            if response["type"] == "response" && response["requestId"] == id {
                if response["resultType"] != "success" {
                    return Err(format!(
                        "Codex IPC: {}",
                        response["error"].as_str().unwrap_or("请求失败")
                    ));
                }
                return Ok(response);
            }
        }
        Err("Codex IPC 响应超时".into())
    }

    pub fn snapshot(
        &mut self,
        thread: &str,
        expected_owner: Option<&str>,
    ) -> Result<Snapshot, String> {
        let params = json!({"hostId":"local", "conversationId":thread});
        let discovery = self.request("thread-owner-discovery", params.clone(), 1, None)?;
        let owner = discovery["handledByClientId"]
            .as_str()
            .ok_or("找不到原桌面任务 owner")?
            .to_owned();
        if expected_owner.is_some_and(|expected| expected != owner) {
            return Err("任务 owner 已变化，请刷新后重试".into());
        }
        self.follow(thread, &owner, true)?;
        let result = self.wait_snapshot(thread, &owner);
        let _ = self.follow(thread, &owner, false);
        result
    }

    fn follow(&mut self, thread: &str, owner: &str, following: bool) -> Result<(), String> {
        self.send(
            &json!({"type":"broadcast", "method":"thread-stream-following-changed",
            "sourceClientId":self.client_id, "targetClientIds":[owner], "version":1,
            "params":{"hostId":"local","conversationId":thread,"following":following}}),
        )
    }

    fn wait_snapshot(&mut self, thread: &str, owner: &str) -> Result<Snapshot, String> {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            let event = self.receive()?;
            self.decline_discovery(&event)?;
            if event["method"] != "thread-stream-state-changed"
                || event["sourceClientId"] != owner
                || event["params"]["conversationId"] != thread
            {
                continue;
            }
            if event["version"] != 11 {
                return Err("Codex 桌面快照协议已变化，请更新 ClawKit".into());
            }
            let change = &event["params"]["change"];
            if change["type"] != "snapshot" {
                continue;
            }
            let state = change["conversationState"].clone();
            if state["id"] != thread {
                return Err("原任务快照身份不匹配".into());
            }
            return Ok(Snapshot {
                owner: owner.into(),
                revision: change["revision"].as_u64().ok_or("快照缺少版本")?,
                state,
            });
        }
        Err("原桌面任务尚未加载，请先在电脑打开该任务".into())
    }

    pub fn start_turn(
        &mut self,
        thread: &str,
        owner: &str,
        request_id: &str,
        text: &str,
    ) -> Result<Value, String> {
        self.request("thread-follower-start-turn", json!({"conversationId":thread,
            "turnStart":{"request":{"threadId":thread,"input":[{"type":"text","text":text,"text_elements":[]}],
                "clientUserMessageId":request_id},"context":{"inheritThreadSettings":true}}}), 2, Some(owner))
    }
}
