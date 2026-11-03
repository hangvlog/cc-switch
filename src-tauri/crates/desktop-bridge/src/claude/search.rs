//! Search the messages you sent in Claude desktop sessions. Transcripts only
//! grow, so each file is parsed once and then read from its last offset.
use serde_json::Value;
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Default)]
struct Indexed {
    offset: u64,
    messages: Vec<(String, String)>,
}

#[derive(Default)]
pub struct Search(Mutex<HashMap<PathBuf, Indexed>>);

fn user_message(line: &str) -> Option<String> {
    if !line.contains("\"type\":\"user\"") && !line.contains("queued_command") {
        return None;
    }
    let record: Value = serde_json::from_str(line).ok()?;
    super::projection::sent_text(&record)
}

fn refresh(entry: &mut Indexed, path: &Path) -> Result<(), String> {
    let len = path.metadata().map_err(|_| "会话记录不可用")?.len();
    if len < entry.offset {
        *entry = Indexed::default();
    }
    let mut file = File::open(path).map_err(|_| "会话记录不可用")?;
    file.seek(SeekFrom::Start(entry.offset)).map_err(|_| "会话记录不可用")?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line).map_err(|_| "会话记录不可用")?;
        // A line still being written has no newline yet; read it next time.
        if read == 0 || !line.ends_with('\n') {
            return Ok(());
        }
        entry.offset += read as u64;
        if let Some(text) = user_message(&line) {
            entry.messages.push((text.to_lowercase(), text));
        }
    }
}

pub struct Hit {
    pub snippet: String,
}

impl Search {
    /// Every term must appear in one message; the newest matching message wins.
    pub fn find(&self, sessions: &[(String, PathBuf)], query: &str) -> Result<(HashMap<String, Hit>, usize), String> {
        let query = query.trim().to_lowercase();
        if query.is_empty() || query.chars().count() > 200 {
            return Err("请输入 1 到 200 个字符进行搜索".into());
        }
        let terms: Vec<&str> = query.split_whitespace().collect();
        let mut cache = self.0.lock().map_err(|_| "搜索暂不可用")?;
        let mut hits = HashMap::new();
        let mut unavailable = 0;
        for (id, path) in sessions {
            let entry = cache.entry(path.clone()).or_default();
            if refresh(entry, path).is_err() {
                unavailable += 1;
                continue;
            }
            if let Some((_, text)) = entry.messages.iter().rev().find(|(lower, _)| terms.iter().all(|t| lower.contains(t))) {
                hits.insert(id.clone(), Hit { snippet: crate::search::snippet(text, terms[0]) });
            }
        }
        Ok((hits, unavailable))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn finds_sent_messages_and_reads_appends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let mut file = File::create(&path).unwrap();
        writeln!(file, r#"{{"type":"user","message":{{"role":"user","content":"部署 共享剪贴板 到 HANG"}}}}"#).unwrap();
        writeln!(file, r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"剪贴板 done"}}]}}}}"#).unwrap();
        writeln!(file, r#"{{"type":"user","isMeta":true,"message":{{"role":"user","content":"剪贴板 meta"}}}}"#).unwrap();
        let search = Search::default();
        let sessions = vec![("local_a".to_string(), path.clone())];
        let (hits, _) = search.find(&sessions, "剪贴板 hang").unwrap();
        assert!(hits["local_a"].snippet.contains("共享剪贴板"));
        assert!(search.find(&sessions, "done").unwrap().0.is_empty());
        write!(file, r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"新的 关键词"}}]}}}}"#).unwrap();
        assert!(search.find(&sessions, "关键词").unwrap().0.is_empty(), "partial line must wait");
        writeln!(file).unwrap();
        assert!(search.find(&sessions, "关键词").unwrap().0.contains_key("local_a"));
    }
}
