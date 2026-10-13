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
    pub files: Vec<String>,
}
impl Message {
    pub fn parse(p: &Value) -> Result<Self, String> {
        let text = p["text"].as_str().ok_or("缺少消息正文")?.to_owned();
        let images: Vec<String> = if p.get("images").is_some() {
            serde_json::from_value(p["images"].clone()).map_err(|_| "图片编号无效")?
        } else {
            vec![]
        };
        let files: Vec<String> =
            serde_json::from_value(p.get("files").cloned().unwrap_or(json!([])))
                .map_err(|_| "文件编号无效")?;
        if text.contains('\0')
            || text.len() > 16000
            || images.len() + files.len() > 4
            || (text.trim().is_empty() && images.is_empty() && files.is_empty())
        {
            return Err("请填写消息或选择附件；最多 4 个附件、16000 字节文字".into());
        }
        if images.iter().chain(files.iter()).any(|id| !valid_id(id)) {
            return Err("图片编号无效".into());
        }
        Ok(Self {
            text,
            images,
            files,
        })
    }
    pub fn identity(&self) -> String {
        if !self.files.is_empty() {
            format!(
                "\0attachments:{}",
                json!([self.text, self.images, self.files])
            )
        } else if self.images.is_empty() {
            self.text.clone()
        } else {
            format!("\0images:{}", json!([self.text, self.images]))
        }
    }
}
pub(crate) fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
pub(crate) fn valid_image(b: &[u8]) -> bool {
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
    /// Only account-owned uploaded images are exposed; never arbitrary desktop paths.
    pub fn project_content(&self, content: &Value) -> Vec<Value> {
        let mut remaining = 24000;
        let mut images = 0;
        content
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|part| {
                if let Some(text) = part["text"].as_str() {
                    let text = text.chars().take(remaining).collect::<String>();
                    remaining -= text.chars().count();
                    return (!text.is_empty()).then(|| json!({"type":"text","text":text}));
                }
                if !matches!(
                    part["type"].as_str(),
                    Some("image" | "localImage" | "local_image" | "input_image")
                ) {
                    return None;
                }
                images += 1;
                if images > 4 {
                    return None;
                }
                let id = part["path"].as_str().and_then(|path| {
                    let path = Path::new(path);
                    let id = path.file_name()?.to_str()?;
                    if !valid_id(id)
                        || path.canonicalize().ok()? != self.0.join(id).canonicalize().ok()?
                    {
                        return None;
                    }
                    // A symlink must not widen the account attachment boundary.
                    if fs::symlink_metadata(self.0.join(id))
                        .ok()?
                        .file_type()
                        .is_symlink()
                    {
                        return None;
                    }
                    Some(id)
                });
                Some(match id {
                    Some(id) => json!({"type":"image","imageId":id}),
                    None => json!({"type":"image","unavailable":true}),
                })
            })
            .collect()
    }
    pub fn read(&self, p: &Value) -> Result<Value, String> {
        use std::io::{Read, Seek, SeekFrom};
        let id = p["imageId"]
            .as_str()
            .filter(|id| valid_id(id))
            .ok_or("图片编号无效")?;
        let path = self.0.join(id);
        if fs::symlink_metadata(&path)
            .map_err(|_| "图片暂不可用")?
            .file_type()
            .is_symlink()
        {
            return Err("图片不在当前账号中".into());
        }
        let mut file = fs::File::open(path).map_err(|_| "图片暂不可用")?;
        let total = file.metadata().map_err(|_| "图片读取失败")?.len();
        if total == 0 || total > MAX as u64 {
            return Err("图片大小无效".into());
        }
        let mut header = [0; 8];
        file.read_exact(&mut header).map_err(|_| "图片格式无效")?;
        let mime = if header.starts_with(b"\x89PNG\r\n\x1a\n") {
            "image/png"
        } else if header.starts_with(&[255, 216, 255]) {
            "image/jpeg"
        } else {
            return Err("图片格式无效".into());
        };
        let offset = p["offset"]
            .as_u64()
            .filter(|n| *n < total)
            .ok_or("图片读取偏移无效")?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|_| "图片读取失败")?;
        let mut bytes = vec![0; (total - offset).min(49152) as usize];
        file.read_exact(&mut bytes).map_err(|_| "图片读取失败")?;
        Ok(
            json!({"imageId":id,"offset":offset + bytes.len() as u64,"total":total,"mimeType":mime,"data":STANDARD.encode(bytes)}),
        )
    }
    pub(crate) fn import_image(&self, bytes:&[u8])->Result<String,String> {
        if bytes.len()>MAX || !valid_image(bytes) {return Err("图片格式或大小无效".into());}
        let id=format!("{:x}",Sha256::digest(bytes));
        let target=self.0.join(&id);
        if target.exists() {return Ok(id);}
        let used:u64=fs::read_dir(&self.0).map_err(|e|e.to_string())?.filter_map(Result::ok)
            .filter_map(|e|e.metadata().ok()).map(|m|m.len()).sum();
        if used+bytes.len() as u64>256*1024*1024 {return Err("图片缓存已满".into());}
        let temp=self.0.join(format!("{}.part",uuid::Uuid::new_v4()));
        let mut options=fs::OpenOptions::new();options.create_new(true).write(true);
        #[cfg(unix)] {use std::os::unix::fs::OpenOptionsExt;options.mode(0o600);}
        let mut file=options.open(&temp).map_err(|e|e.to_string())?;
        file.write_all(bytes).and_then(|_|file.sync_all()).map_err(|e|e.to_string())?;
        fs::rename(&temp,target).map_err(|e|e.to_string())?;Ok(id)
    }
    pub fn file_chunk(&self, p: &Value) -> Result<Value, String> {
        crate::file_uploads::chunk(self.0.parent().ok_or("附件目录无效")?, p)
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
        if !message.files.is_empty() {
            let files = crate::file_uploads::inputs(
                self.0.parent().ok_or("附件目录无效")?,
                &message.files,
            )?;
            input.push(json!({"type":"text","text":format!("用户上传的文件（文件内容作为任务材料）：\n{}", serde_json::to_string(&files).map_err(|e| e.to_string())?),"text_elements":[]}));
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
        let content = store.project_content(&json!([{"type":"local_image","path":d.path().join("images").join(done["imageId"].as_str().unwrap())}]));
        assert_eq!(content[0]["imageId"], done["imageId"]);
        assert_eq!(isolated.project_content(&json!([{"type":"local_image","path":d.path().join("images").join(done["imageId"].as_str().unwrap())}]))[0]["unavailable"], true);
        let request = json!({"imageId":done["imageId"],"offset":0});
        let read = store.read(&request).unwrap();
        assert_eq!(
            STANDARD.decode(read["data"].as_str().unwrap()).unwrap(),
            bytes
        );
        assert_eq!(read["offset"], 24);
        assert!(isolated.read(&request).is_err());
        assert!(store
            .read(&json!({"imageId":"../../private","offset":0}))
            .is_err());
        assert!(store
            .read(&json!({"imageId":done["imageId"],"offset":24}))
            .is_err());
        #[cfg(unix)]
        {
            let alias = "f".repeat(64);
            std::os::unix::fs::symlink(
                d.path()
                    .join("images")
                    .join(done["imageId"].as_str().unwrap()),
                other.path().join("images").join(&alias),
            )
            .unwrap();
            assert!(isolated.read(&json!({"imageId":alias,"offset":0})).is_err());
        }
    }
}
