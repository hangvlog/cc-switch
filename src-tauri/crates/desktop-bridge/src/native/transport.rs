use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    time::Duration,
};
pub(super) const LOCAL_LIMIT: usize = 64 * 1024;
pub(super) const RESPONSE_LIMIT: usize = 16 * 1024 * 1024;

pub(super) fn read_line(
    reader: &mut impl BufRead,
    limit: usize,
) -> Result<Option<Vec<u8>>, String> {
    let mut bytes = Vec::new();
    reader
        .take((limit + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| "原生连接读取失败")?;
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.len() > limit || bytes.last() != Some(&b'\n') {
        return Err("原生协议消息超出范围或不完整".into());
    }
    Ok(Some(bytes))
}

#[cfg(unix)]
pub(super) fn check_private(path: &Path, directory: bool) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path)
        .map_err(|_| "新建通道尚未接入当前 Codex；请检查电脑远控设置，草稿未发送")?;
    if meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
        || meta.file_type().is_symlink()
        || (directory && !meta.is_dir())
    {
        return Err("原生通道文件权限无效".into());
    }
    Ok(())
}

pub(crate) struct Failure { pub definite: bool, pub message: String }
impl From<String> for Failure {fn from(message:String)->Self {Self{message,definite:false}}}
impl From<&str> for Failure {fn from(message:&str)->Self {message.to_owned().into()}}

pub(super) fn request(socket: &Path, method: &str, params: Value) -> Result<Value, Failure> {
    #[cfg(unix)] {check_private(socket.parent().ok_or("通道路径无效")?, true)?; check_private(socket, false)?;}
    let mut stream =
        super::endpoint::Stream::connect(socket).map_err(|_| "新建通道尚未接入当前 Codex；请检查电脑远控设置，草稿未发送")?;
    stream
        .set_read_timeout(Some(Duration::from_secs(if method == "clawkit/status" {
            2
        } else {
            35
        })))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let bytes =
        serde_json::to_vec(&json!({"method":method,"params":params})).map_err(|e| e.to_string())?;
    if bytes.len() > LOCAL_LIMIT - 1 {
        return Err("原生请求过大".into());
    }
    stream
        .write_all(&bytes)
        .and_then(|_| stream.write_all(b"\n"))
        .map_err(|_| "原生请求发送结果未知")?;
    let raw = read_line(&mut BufReader::new(stream), RESPONSE_LIMIT)?.ok_or("原生请求结果未知")?;
    let response: Value = serde_json::from_slice(&raw).map_err(|_| "原生响应格式不兼容")?;
    if let Some(error) = response.get("error") {
        let code = error["code"].as_i64().unwrap_or(0);
        let reason = match (code,error["data"]["type"].as_str()) {
            (_,Some("modelProviderNotConfigured"))=>"此对话的模型供应商未配置，请在电脑恢复原供应商",
            (-32601,_)=>"桌面端暂不支持此操作，请更新 ClawKit 并在方便时重开 Codex",
            (-32001,_)=>"Codex 正在重新连接，请稍后刷新",
            (-32002,_)=>"等待电脑响应超时，请刷新核对状态",
            (-32004,_)=>"对话内容超出单页大小，请刷新后分段查看",
            (-32003,_)=>"待处理请求已变化，请刷新后核对",
            (-32602,_)=>"当前 Codex 不接受此项参数或所选轮次，请刷新后核对",
            _=>"电脑未完成此操作，请刷新对话核对状态",
        };
        return Err(Failure{message:format!("{reason}（{method}，{code}）"),
            definite: matches!(code,-32600|-32601|-32602|-32001|-32003)});
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| "原生响应缺少结果".into())
}
