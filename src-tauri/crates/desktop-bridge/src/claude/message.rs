//! Claude stream-json accepts image blocks. Its authenticated peer inbox accepts only text.
use crate::attachments::{valid_id, Attachments};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{fs, path::{Path, PathBuf}};

const IMAGES: &str = "\n\nClawKit 手机图片，请使用 Read 查看原图：\n";

pub(super) struct Message {
    pub text: String,
    images: Vec<(PathBuf, Vec<u8>, &'static str)>,
}

impl Message {
    pub fn parse(params: &Value, store: &Attachments) -> Result<Self, String> {
        let text = params["text"].as_str().unwrap_or("").trim();
        let ids: Vec<String> = serde_json::from_value(params.get("images").cloned().unwrap_or(json!([])))
            .map_err(|_| "图片编号无效")?;
        if ids.is_empty() && text.is_empty() { return Err("请输入消息或选择图片".into()); }
        if text.len() > super::MAX_MESSAGE || ids.len() > 4 { return Err("消息过长或图片超过 4 张".into()); }
        let images = ids.iter().map(|id| store.claude_image(id)).collect::<Result<_, _>>()?;
        Ok(Self { text: text.into(), images })
    }

    pub fn content(&self) -> Value {
        if self.images.is_empty() { return json!(self.text); }
        let mut blocks = Vec::new();
        for (_, data, mime) in &self.images {
            blocks.push(json!({"type":"image","source":{"type":"base64","media_type":mime,"data":STANDARD.encode(data)}}));
        }
        if !self.text.is_empty() { blocks.push(json!({"type":"text","text":self.text})); }
        json!(blocks)
    }

    pub fn peer_text(&self, request: &str) -> Result<String, String> {
        let mut prompt = self.text.clone();
        if !self.images.is_empty() {
            let mut paths = Vec::new();
            for (original, bytes, mime) in &self.images {
                // Read selects its image decoder by extension. Both names refer to the same private upload.
                let path = original.with_extension(if *mime == "image/png" { "png" } else { "jpg" });
                match fs::hard_link(original, &path) {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) ||
                            fs::read(&path).ok().as_ref() != Some(bytes) {
                            return Err("图片引用校验失败".into());
                        }
                    }
                    Err(_) => return Err("无法为 Claude 准备图片".into()),
                }
                paths.push(path);
            }
            prompt.push_str(IMAGES);
            prompt.push_str(&json!(paths).to_string());
        }
        Ok(format!("{prompt}\n[ClawKit:{request}]"))
    }
}

/// Parse only the suffix of an authenticated ClawKit peer message.
pub fn peer_parts(text: &str) -> (&str, Vec<String>, Option<&str>) {
    let Some((body, end)) = text.trim_end().rsplit_once("\n[ClawKit:") else { return (text, vec![], None); };
    let Some(id) = end.strip_suffix(']').filter(|v| super::index::is_uuid(v)) else { return (text, vec![], None); };
    let Some((caption, paths)) = body.rsplit_once(IMAGES) else { return (body, vec![], Some(id)); };
    let Ok(paths) = serde_json::from_str::<Vec<String>>(paths) else { return (body, vec![], Some(id)); };
    let ids = paths.iter().take(4).filter_map(|p| Path::new(p).file_stem()?.to_str().filter(|v| valid_id(v)).map(String::from)).collect();
    (caption, ids, Some(id))
}

pub fn history_images(blocks: &Value) -> Vec<Value> {
    use sha2::{Digest, Sha256};
    blocks.as_array().into_iter().flatten().filter(|b| b["type"] == "image").take(4).map(|b| {
        let id = b["source"]["data"].as_str().filter(|s| s.len() <= 7_000_000)
            .and_then(|s| STANDARD.decode(s).ok()).map(|bytes| format!("{:x}", Sha256::digest(bytes)));
        match id {
            Some(id) => json!({"type":"image","imageId":id}),
            None => json!({"type":"image","unavailable":true}),
        }
    }).collect()
}
