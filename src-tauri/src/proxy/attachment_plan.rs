//! Native Responses attachments only. Preserve every non-attachment field and original bytes.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

const URL_RESERVE: usize = 768;
pub(super) struct Attachment {
    pub hash: String,
    pub mime: String,
    pub bytes: Vec<u8>,
    pub paths: Vec<(String, bool)>,
    pub saved: usize,
}
fn candidates(value: &Value, path: &str, out: &mut Vec<(String, String, String, bool, usize)>) {
    match value {
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                candidates(value, &format!("{path}/{index}"), out);
            }
        }
        Value::Object(object) => {
            let (field, file) = match object.get("type").and_then(Value::as_str) {
                Some("input_image") => ("image_url", false),
                Some("input_file") => ("file_data", true),
                _ => ("", false),
            };
            // Native Responses file_url is mutually exclusive with filename.
            // Keep named inline files unchanged; removing names would alter client fields.
            let url_compatible = !file || !object.contains_key("filename");
            if let Some(raw) = object.get(field).and_then(Value::as_str).filter(|_| url_compatible) {
                let parsed = raw
                    .strip_prefix("data:")
                    .and_then(|data| {
                        let (mime, data) = data.split_once(";base64,")?;
                        Some((mime, data))
                    })
                    .or_else(|| {
                        (file && raw.starts_with("JVBERi0")).then_some(("application/pdf", raw))
                    });
                if let Some((mime, data)) = parsed {
                    if ((file && mime == "application/pdf")
                        || (!file
                            && matches!(
                                mime,
                                "image/png" | "image/jpeg" | "image/webp" | "image/gif"
                            )))
                        && raw.len() > URL_RESERVE
                    {
                        out.push((
                            format!("{path}/{field}"),
                            mime.into(),
                            data.into(),
                            file,
                            raw.len(),
                        ));
                    }
                }
            }
            // Traverse protocol containers, never tool schemas or arbitrary parameter objects.
            for field in ["content", "output"] {
                if let Some(value) = object.get(field) {
                    candidates(value, &format!("{path}/{field}"), out);
                }
            }
        }
        _ => {}
    }
}
pub(super) fn plan(body: &Value, max_attachment: usize) -> Result<Vec<Attachment>, String> {
    let mut raw = Vec::new();
    if let Some(input) = body.get("input") {
        candidates(input, "/input", &mut raw);
    }
    let mut groups: Vec<Attachment> = Vec::new();
    let mut indexes = HashMap::new();
    for (path, mime, data, file, original_size) in raw {
        if data.len() > max_attachment.saturating_mul(4) / 3 + 4 {
            continue;
        }
        let bytes = STANDARD
            .decode(data)
            .map_err(|_| "Attachment Base64 is invalid")?;
        if bytes.is_empty() || bytes.len() > max_attachment {
            continue;
        }
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let key = format!("{hash}:{mime}");
        let index = match indexes.get(&key) {
            Some(index) => *index,
            None => {
                let index = groups.len();
                indexes.insert(key, index);
                groups.push(Attachment {
                    hash,
                    mime,
                    bytes,
                    paths: Vec::new(),
                    saved: 0,
                });
                index
            }
        };
        groups[index].paths.push((path, file));
        groups[index].saved += original_size.saturating_sub(URL_RESERVE);
    }
    Ok(groups)
}
pub(super) fn replace(body: &mut Value, attachment: &Attachment, url: &str) -> Result<(), String> {
    for (path, file) in &attachment.paths {
        if *file {
            let (parent, _) = path.rsplit_once('/').ok_or("Invalid attachment location")?;
            let object = body
                .pointer_mut(parent)
                .and_then(Value::as_object_mut)
                .ok_or("Attachment disappeared")?;
            object.remove("file_data");
            object.insert("file_url".into(), Value::String(url.into()));
        } else {
            *body.pointer_mut(path).ok_or("Attachment disappeared")? = Value::String(url.into());
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn tool_images_are_deduplicated_and_other_fields_preserved() {
        let data = format!("data:image/png;base64,{}", STANDARD.encode(vec![5; 4096]));
        let body = json!({"model":"gpt-6.1-sol","stream":true,"input":[{"type":"custom_tool_call_output","id":"ctco_one","call_id":"call_one","output":[{"type":"input_text","text":"keep"},{"type":"input_image","image_url":data,"detail":"high"},{"type":"input_image","image_url":data}]}],"tools":[{"type":"namespace","name":"keep"}]});
        let planned = plan(&body, 20 << 20).unwrap();
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].paths.len(), 2);
        let mut updated = body.clone();
        replace(&mut updated, &planned[0], "https://example.test/signed").unwrap();
        assert_eq!(updated["input"][0]["output"][1]["detail"], "high");
        assert_eq!(updated["tools"], body["tools"]);
        assert_eq!(updated["input"][0]["id"], "ctco_one");
        assert_ne!(updated, body);
        assert!(body["input"][0]["output"][1]["image_url"]
            .as_str()
            .unwrap()
            .starts_with("data:"));
    }
    #[test]
    fn named_pdf_is_preserved_and_unnamed_pdf_can_use_url() {
        let raw = STANDARD.encode([b"%PDF-".as_slice(), vec![0; 4096].as_slice()].concat());
        let mut body = json!({"input":[{"role":"user","content":[{"type":"input_file","file_data":raw,"filename":"report.pdf"}]}]});
        assert!(plan(&body, 20 << 20).unwrap().is_empty());
        assert_eq!(body["input"][0]["content"][0]["filename"], "report.pdf");
        body["input"][0]["content"][0].as_object_mut().unwrap().remove("filename");
        let planned = plan(&body, 20 << 20).unwrap();
        replace(&mut body, &planned[0], "https://example.test/file").unwrap();
        assert!(body["input"][0]["content"][0].get("file_data").is_none());
        assert_eq!(
            body["input"][0]["content"][0]["file_url"],
            "https://example.test/file"
        );
    }
}
