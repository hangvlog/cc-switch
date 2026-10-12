//! Account-scoped, bounded uploads. Callers may reference only content stored here.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
const MAX: usize = 5 * 1024 * 1024;
pub struct Attachments(PathBuf);
pub struct Message {
    pub text: String,
    pub images: Vec<String>,
}
impl Message {
    pub fn parse(p: &Value) -> Result<Self, String> {
        let text = p["text"].as_str().ok_or("缺少消息正文")?.to_owned();
        let images: Vec<String> = if p.get("images").is_some() {
            serde_json::from_value(p["images"].clone()).map_err(|_| "图片编号无效")?
        } else {
            vec![]
        };
        if text.contains('\0')
            || text.len() > 16000
            || images.len() > 4
            || (text.trim().is_empty() && images.is_empty())
        {
            return Err("请填写消息或选择图片；最多 4 张图片、16000 字节文字".into());
        }
        if images.iter().any(|id| !valid_id(id)) {
            return Err("图片编号无效".into());
        }
        Ok(Self { text, images })
    }
    pub fn identity(&self) -> String {
        if self.images.is_empty() {
            self.text.clone()
        } else {
            format!("\0images:{}", json!([self.text, self.images]))
        }
    }
}
fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn valid_image(b: &[u8]) -> bool {
    (b.starts_with(b"\x89PNG\r\n\x1a\n") && b.len() >= 24)
        || (b.starts_with(&[255, 216, 255]) && b.ends_with(&[255, 217]))
}
impl Attachments {
    pub fn open(storage: &Path) -> Result<Self, String> {
        let root = storage.join("images");
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        Ok(Self(root))
    }
    pub fn chunk(&self, p: &Value) -> Result<Value, String> {
        let upload = p["uploadId"].as_str().ok_or("缺少上传编号")?;
        let uuid = uuid::Uuid::parse_str(upload).map_err(|_| "上传编号无效")?;
        let offset = p["offset"].as_u64().ok_or("上传偏移无效")? as usize;
        let total = p["total"]
            .as_u64()
            .filter(|n| *n > 0 && *n <= MAX as u64)
            .ok_or("图片最大 5 MB")? as usize;
        let encoded = p["data"]
            .as_str()
            .filter(|s| s.len() <= 24576)
            .ok_or("图片分块过大")?;
        let data = STANDARD.decode(encoded).map_err(|_| "图片编码无效")?;
        if data.is_empty() || offset + data.len() > total {
            return Err("图片分块范围无效".into());
        }
        let file = self.0.join(format!("{}.part", uuid));
        let mut bytes = if file.exists() {
            fs::read(&file).map_err(|e| e.to_string())?
        } else {
            vec![]
        };
        if bytes.len() > total {
            return Err("上传总长度改变，请重新选择图片".into());
        }
        if offset > bytes.len() {
            return Err("图片上传不连续，请重新发送".into());
        }
        if offset < bytes.len() {
            if bytes.get(offset..offset + data.len()) != Some(data.as_slice()) {
                return Err("上传编号已用于不同图片".into());
            }
        } else {
            let used: u64 = fs::read_dir(&self.0)
                .map_err(|e| e.to_string())?
                .filter_map(Result::ok)
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum();
            if used + data.len() as u64 > 256 * 1024 * 1024 {
                return Err("电脑图片缓存已满，请在电脑清理后重试".into());
            }
            let mut opts = fs::OpenOptions::new();
            opts.create(true).append(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut output = opts.open(&file).map_err(|e| e.to_string())?;
            output.write_all(&data).map_err(|e| e.to_string())?;
            output.sync_all().map_err(|e| e.to_string())?;
            bytes.extend_from_slice(&data);
        }
        let acknowledged = offset + data.len();
        if acknowledged != total {
            return Ok(json!({"offset":acknowledged}));
        }
        if !valid_image(&bytes) {
            return Err("请选择 JPEG 或 PNG 图片".into());
        }
        let id = format!("{:x}", Sha256::digest(&bytes));
        // Completed content is immutable. Keep the part for retries after a lost final reply.
        let target = self.0.join(&id);
        if !target.exists() {
            fs::copy(&file, target).map_err(|e| e.to_string())?;
        }
        Ok(json!({"offset":total,"imageId":id}))
    }
    pub fn input(&self, message: &Message) -> Result<Value, String> {
        let mut input = vec![];
        if !message.text.trim().is_empty() {
            input.push(json!({"type":"text","text":message.text,"text_elements":[]}));
        }
        for id in &message.images {
            if !valid_id(id) {
                return Err("图片编号无效".into());
            }
            let path = self.0.join(id);
            if !path.is_file() {
                return Err("图片尚未传到此电脑，请重新发送".into());
            }
            input.push(json!({"type":"localImage","path":path}));
        }
        Ok(json!(input))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upload_retry_scope_and_image_only() {
        let d = tempfile::tempdir().unwrap();
        let store = Attachments::open(d.path()).unwrap();
        let other = tempfile::tempdir().unwrap();
        let isolated = Attachments::open(other.path()).unwrap();
        let bytes = b"\x89PNG\r\n\x1a\nabcdefghijklmnop";
        let upload = uuid::Uuid::new_v4().to_string();
        let a =
            json!({"uploadId":upload,"offset":0,"total":24,"data":STANDARD.encode(&bytes[..12])});
        assert_eq!(store.chunk(&a).unwrap()["offset"], 12);
        assert_eq!(store.chunk(&a).unwrap()["offset"], 12);
        let b =
            json!({"uploadId":upload,"offset":12,"total":24,"data":STANDARD.encode(&bytes[12..])});
        let done = store.chunk(&b).unwrap();
        assert_eq!(store.chunk(&b).unwrap(), done);
        let msg = Message::parse(&json!({"text":"","images":[done["imageId"]]})).unwrap();
        assert_eq!(store.input(&msg).unwrap()[0]["type"], "localImage");
        assert!(isolated.input(&msg).is_err());
        assert!(Message::parse(&json!({"text":"","images":["../../secret"]})).is_err());
        assert_ne!(msg.identity(), "");
    }
}
