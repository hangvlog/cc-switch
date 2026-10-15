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

pub(super) fn request(socket: &Path, method: &str, params: Value) -> Result<Value, String> {
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
    if response.get("error").is_some() {
        return Err("Codex 拒绝原生请求，请检查桌面状态；不会自动重试".into());
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| "原生响应缺少结果".into())
}
