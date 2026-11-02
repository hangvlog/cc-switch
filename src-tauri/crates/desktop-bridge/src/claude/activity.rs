//! When a conversation last had a message. The CLI appends untimestamped
//! bookkeeping records (`last-prompt`, `mode`, `cost-state`) whenever a
//! session is opened or resumed, so the file's mtime is not activity.
use serde_json::Value;
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::UNIX_EPOCH,
};

const FIRST_TAIL: u64 = 64 * 1024;
const MAX_TAIL: u64 = 8 * 1024 * 1024;

type Cache = HashMap<PathBuf, (u64, u64, u64)>;

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// `2026-11-02T11:47:23.135Z` -> unix ms. Only the UTC form the CLI writes.
pub fn parse_iso_ms(text: &str) -> Option<u64> {
    let b = text.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || !text.ends_with('Z') {
        return None;
    }
    let num = |r: std::ops::Range<usize>| text.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let millis = text.get(19..text.len() - 1).filter(|f| f.starts_with('.')).map_or(Some(0), |f| {
        let digits = &f[1..];
        let value: i64 = digits.get(..digits.len().min(3))?.parse().ok()?;
        Some(value * 10_i64.pow(3 - digits.len().min(3) as u32))
    })?;
    let (yy, mon) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * mon + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let ms = ((days * 24 + hh) * 60 + mm) * 60_000 + ss * 1000 + millis;
    u64::try_from(ms).ok()
}

fn scan(chunk: &[u8], partial_first_line: bool) -> Option<u64> {
    let mut lines: Vec<&[u8]> = chunk.split(|&c| c == b'\n').collect();
    if partial_first_line && !lines.is_empty() {
        lines.remove(0);
    }
    lines.iter().rev().find_map(|line| {
        let record: Value = serde_json::from_slice(line).ok()?;
        if !matches!(record["type"].as_str(), Some("user" | "assistant")) || record["isSidechain"] == true {
            return None;
        }
        parse_iso_ms(record["timestamp"].as_str()?)
    })
}

fn read_tail(path: &Path, len: u64) -> u64 {
    let Ok(mut file) = File::open(path) else { return 0 };
    let mut size = FIRST_TAIL;
    loop {
        let start = len.saturating_sub(size);
        let mut chunk = Vec::with_capacity((len - start) as usize);
        if file.seek(SeekFrom::Start(start)).is_err() || file.by_ref().take(len - start).read_to_end(&mut chunk).is_err() {
            return 0;
        }
        if let Some(ms) = scan(&chunk, start > 0) {
            return ms;
        }
        if start == 0 || size >= MAX_TAIL {
            return 0;
        }
        size *= 4;
    }
}

/// Last user/assistant message time, cached by file size and mtime.
pub fn last_message_ms(path: &Path) -> u64 {
    let Ok(meta) = path.metadata() else { return 0 };
    let len = meta.len();
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64);
    if let Some(&(l, m, ms)) = cache().lock().ok().as_ref().and_then(|c| c.get(path)) {
        if l == len && m == mtime {
            return ms;
        }
    }
    let ms = read_tail(path, len);
    if let Ok(mut c) = cache().lock() {
        c.insert(path.to_path_buf(), (len, mtime, ms));
    }
    ms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cli_timestamps() {
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(parse_iso_ms("2026-11-02T11:47:23.135Z"), Some(1_793_620_043_135));
        assert_eq!(parse_iso_ms("2024-02-29T23:59:59Z"), Some(1_709_251_199_000));
        assert_eq!(parse_iso_ms("not a time"), None);
    }

    #[test]
    fn ignores_trailing_bookkeeping_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let filler = format!("{{\"type\":\"progress\",\"data\":\"{}\"}}\n", "x".repeat(100_000));
        std::fs::write(&path, format!(
            "{{\"type\":\"user\",\"timestamp\":\"2026-09-08T02:29:50.465Z\"}}\n{filler}{{\"type\":\"assistant\",\"isSidechain\":true,\"timestamp\":\"2026-10-01T00:00:00.000Z\"}}\n{{\"type\":\"last-prompt\",\"lastPrompt\":\"x\"}}\n{{\"type\":\"cost-state\"}}\n"
        )).unwrap();
        assert_eq!(last_message_ms(&path), 1_788_834_590_465);
    }
}
