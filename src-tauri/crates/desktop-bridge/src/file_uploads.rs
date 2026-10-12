//! System-picked documents, scoped to the logged-in account. Never accepts caller paths.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
const MAX: u64 = 20 * 1024 * 1024;
const QUOTA: u64 = 256 * 1024 * 1024;
fn private_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path).map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}
fn name(p: &Value) -> Result<&str, String> {
    p["name"]
        .as_str()
        .filter(|n| {
            !n.trim().is_empty()
                && n.len() <= 200
                && !n.ends_with(['.', ' '])
                && !n
                    .chars()
                    .any(|c| c.is_control() || "\\/:*?\"<>|".contains(c))
                && *n != "."
                && *n != ".."
        })
        .ok_or_else(|| "文件名无效，请重命名后选择".into())
}
fn used(path: &Path) -> u64 {
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| {
            if e.path().is_dir() {
                used(&e.path())
            } else {
                e.metadata().map(|m| m.len()).unwrap_or(0)
            }
        })
        .sum()
}
pub fn chunk(storage: &Path, p: &Value) -> Result<Value, String> {
    let root = storage.join("files");
    private_dir(&root)?;
    let upload = uuid::Uuid::parse_str(p["uploadId"].as_str().ok_or("缺少上传编号")?)
        .map_err(|_| "上传编号无效")?;
    let name = name(p)?;
    let total = p["total"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= MAX)
        .ok_or("文件最大 20 MB，且不能为空")?;
    let offset = p["offset"]
        .as_u64()
        .filter(|n| *n < total)
        .ok_or("上传偏移无效")?;
    let data = STANDARD
        .decode(
            p["data"]
                .as_str()
                .filter(|s| s.len() <= 24576)
                .ok_or("文件分块过大")?,
        )
        .map_err(|_| "文件编码无效")?;
    let end = offset + data.len() as u64;
    if data.is_empty() || end > total {
        return Err("文件分块范围无效".into());
    }
    let meta = root.join(format!("{upload}.json"));
    let manifest = json!({"name":name,"total":total});
    if meta.exists() {
        let prior: Value = serde_json::from_slice(&fs::read(&meta).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        if prior != manifest {
            return Err("上传编号已用于不同文件".into());
        }
    } else {
        // Reserve space for both partial and final copy before accepting a new upload.
        if offset != 0 {
            return Err("文件上传不连续".into());
        }
        if used(&root) + total * 2 > QUOTA {
            return Err("电脑文件缓存已满，请在电脑清理后重试".into());
        }
        write_new(&meta, manifest.to_string().as_bytes())?;
    }
    let part = root.join(format!("{upload}.part"));
    if !part.exists() {
        write_new(&part, &[])?;
    }
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&part)
        .map_err(|e| e.to_string())?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    if offset > len || len > total {
        return Err("文件上传不连续".into());
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| e.to_string())?;
    if offset < len {
        let mut old = vec![0; data.len()];
        file.read_exact(&mut old).map_err(|_| "文件分块重叠")?;
        if old != data {
            return Err("上传编号已用于不同文件".into());
        }
    } else {
        if used(&root) + data.len() as u64 > QUOTA {
            return Err("电脑文件缓存已满".into());
        }
        file.write_all(&data)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
    }
    if end != total {
        return Ok(json!({"offset":end}));
    }
    let mut digest = Sha256::new();
    digest.update(name.as_bytes());
    digest.update([0]);
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut buffer = [0u8; 16384];
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    let id = format!("{:x}", digest.finalize());
    let dir = root.join(&id);
    private_dir(&dir)?;
    // Prefix also avoids reserved Windows device basenames, while retaining the original extension.
    let target = dir.join(format!("attachment-{name}"));
    if !target.exists() {
        if used(&root) + total > QUOTA {
            return Err("电脑文件缓存已满".into());
        }
        fs::copy(&part, &target).map_err(|e| e.to_string())?;
    }
    let final_meta = dir.join("metadata.json");
    if !final_meta.exists() {
        write_new(&final_meta, manifest.to_string().as_bytes())?;
    }
    Ok(json!({"offset":end,"fileId":id}))
}
pub fn inputs(storage: &Path, ids: &[String]) -> Result<Vec<Value>, String> {
    ids.iter()
        .map(|id| {
            if id.len() != 64
                || !id
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            {
                return Err("文件编号无效".into());
            }
            let dir = storage.join("files").join(id);
            let meta: Value = serde_json::from_slice(
                &fs::read(dir.join("metadata.json")).map_err(|_| "文件尚未传到此电脑")?,
            )
            .map_err(|e| e.to_string())?;
            let name = name(&meta)?;
            let path: PathBuf = dir.join(format!("attachment-{name}"));
            if !path.is_file() {
                return Err("文件尚未传到此电脑".into());
            }
            Ok(json!({"name":name,"path":path}))
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_upload_retries_and_scopes() {
        let d = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let bytes = b"document contents";
        let upload = uuid::Uuid::new_v4().to_string();
        let p = json!({"uploadId":upload,"name":"report.txt","offset":0,"total":bytes.len(),"data":STANDARD.encode(bytes)});
        let done = chunk(d.path(), &p).unwrap();
        assert_eq!(chunk(d.path(), &p).unwrap(), done);
        let ids = vec![done["fileId"].as_str().unwrap().to_owned()];
        let files = inputs(d.path(), &ids).unwrap();
        assert_eq!(fs::read(files[0]["path"].as_str().unwrap()).unwrap(), bytes);
        assert!(inputs(other.path(), &ids).is_err());
        let mut changed = p.clone();
        changed["name"] = json!("other.txt");
        assert!(chunk(d.path(), &changed).is_err());
        changed["name"] = json!("../report.txt");
        assert!(chunk(d.path(), &changed).is_err());
        changed["name"] = json!("report.txt");
        changed["total"] = json!(MAX + 1);
        assert!(chunk(d.path(), &changed).is_err());
        let message = crate::attachments::Message::parse(&json!({"text":"","files":ids})).unwrap();
        let store = crate::attachments::Attachments::open(d.path()).unwrap();
        let input = store.input(&message).unwrap();
        assert!(input[0]["text"]
            .as_str()
            .unwrap()
            .contains("attachment-report.txt"));
        assert_ne!(message.identity(), "");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(files[0]["path"].as_str().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}
