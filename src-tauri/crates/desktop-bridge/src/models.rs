//! Read the installed CLI's model catalog only; never resume or execute a task here.
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

pub fn list(home: &Path) -> Result<Value, String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = home;
        return Err("当前系统尚不支持读取桌面模型".into());
    }
    #[cfg(target_os = "macos")]
    {
        let contents = crate::version::running_contents(home).ok_or("Codex 桌面尚未运行")?;
        let candidates = [
            contents.join("Resources/codex-cli/bin/codex"),
            contents.join("Resources/codex"),
        ];
        let binary = candidates
            .iter()
            .find(|p| p.is_file())
            .ok_or("找不到桌面附带的模型目录接口")?;
        let mut child = Command::new(binary)
            .arg("app-server")
            .env("CODEX_HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "无法读取 Codex 模型目录")?;
        let mut input = child.stdin.take().ok_or("模型目录输入不可用")?;
        let output = child.stdout.take().ok_or("模型目录输出不可用")?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| -> Result<Value, String> {
                writeln!(input, "{}", json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"clawkit-model-catalog","version":"1"},"capabilities":{}}})).map_err(|e|e.to_string())?;
                input.flush().map_err(|e| e.to_string())?;
                let mut reader = BufReader::new(output);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                        return Err("模型目录连接已关闭".into());
                    }
                    if line.len() > 4 * 1024 * 1024 {
                        return Err("模型目录响应过大".into());
                    }
                    let response: Value =
                        serde_json::from_str(&line).map_err(|_| "模型目录响应不兼容")?;
                    if response["id"] == 1 {
                        if response.get("error").is_some() {
                            return Err("模型目录初始化失败".into());
                        }
                        writeln!(
                            input,
                            "{}\n{}",
                            json!({"method":"initialized"}),
                            json!({"id":2,"method":"model/list","params":{"includeHidden":false}})
                        )
                        .map_err(|e| e.to_string())?;
                        input.flush().map_err(|e| e.to_string())?;
                    } else if response["id"] == 2 {
                        if response.get("error").is_some() {
                            return Err("Codex 暂时无法列出模型".into());
                        }
                        let data = response["result"]["data"]
                            .as_array()
                            .ok_or("模型列表格式不兼容")?;
                        let data: Vec<_> = data.iter().filter(|m|m["model"].is_string()).map(|m|json!({"id":m["model"],"model":m["model"],"displayName":m["displayName"],"isDefault":m["isDefault"]})).collect();
                        return Ok(json!({"data":data}));
                    }
                }
            })();
            let _ = tx.send(result);
        });
        let result = rx
            .recv_timeout(Duration::from_secs(12))
            .map_err(|_| "读取模型目录超时".to_owned())
            .and_then(|v| v);
        let _ = child.kill();
        let _ = child.wait();
        result
    }
}

pub fn select(home: &Path, thread: &str, model: &str) -> Result<Value, String> {
    if !list(home)?["data"]
        .as_array()
        .is_some_and(|data| data.iter().any(|m| m["model"] == model))
    {
        return Err("所选模型不在电脑当前可用列表中，请刷新模型列表".into());
    }
    let mut client = crate::ipc::IpcClient::connect(home)?;
    let snapshot = client.snapshot(thread, None)?;
    if let Some(reason) = crate::history::send_block_reason(&snapshot.state) {
        return Err(reason.into());
    }
    let settings = client.request(
        "thread-follower-read-model-settings",
        json!({"conversationId":thread}),
        1,
        Some(&snapshot.owner),
    )?;
    let current = &settings["result"]["settings"];
    let old_model = current["model"]
        .as_str()
        .ok_or("原任务模型设置响应不兼容")?;
    if old_model == model {
        return Ok(json!({"model":model}));
    }
    let response = client.request(
        "thread-follower-update-thread-settings",
        json!({"conversationId":thread,
        "threadSettings":{"model":model,"effort":null},
        "condition":{"ifModelEquals":old_model,"ifEffortEquals":current["reasoningEffort"]}}),
        2,
        Some(&snapshot.owner),
    );
    // Read back even if the acknowledgement was lost. Never retry the write.
    let mut fresh = crate::ipc::IpcClient::connect(home)?;
    let latest = fresh.request(
        "thread-follower-read-model-settings",
        json!({"conversationId":thread}),
        1,
        Some(&snapshot.owner),
    )?;
    if latest["result"]["settings"]["model"] == model {
        return Ok(json!({"model":model}));
    }
    response?;
    Err("未确认模型切换成功，请刷新对话后重试".into())
}
