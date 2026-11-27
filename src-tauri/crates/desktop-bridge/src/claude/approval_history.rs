//! Correlate an official PermissionRequest with the result on its own transcript.
//! Some CLI versions deliver the result while the local approval hook still waits.
use super::{activity, projection};
use serde_json::Value;
use std::{collections::HashMap, path::Path};

#[derive(Default)]
pub(super) struct Calls {
    uses: HashMap<String, (String, Value, u64)>,
    results: HashMap<String, u64>,
}

pub(super) fn read(path: Option<&Path>) -> Calls {
    path.and_then(|p| projection::read_records(p).ok())
        .map(|records| from_records(&records))
        .unwrap_or_default()
}

pub(super) fn from_records(records: &[Value]) -> Calls {
    let mut calls = Calls::default();
    for row in projection::active_chain(records) {
        let time = row["timestamp"].as_str().and_then(activity::parse_iso_ms);
        let Some(blocks) = row["message"]["content"].as_array() else {
            continue;
        };
        for b in blocks {
            match (row["type"].as_str(), b["type"].as_str()) {
                (Some("assistant"), Some("tool_use")) => {
                    if let (Some(id), Some(name), Some(ms)) =
                        (b["id"].as_str(), b["name"].as_str(), time)
                    {
                        calls
                            .uses
                            .insert(id.to_owned(), (name.to_owned(), b["input"].clone(), ms));
                    }
                }
                (Some("user"), Some("tool_result")) => {
                    if let Some(id) = b["tool_use_id"].as_str() {
                        calls.results.insert(id.to_owned(), time.unwrap_or(0));
                    }
                }
                _ => {}
            }
        }
    }
    calls
}

impl Calls {
    // PermissionRequest has no tool_use_id. The hook may start long after a
    // tool call was written. Match the call intervals that intersect its
    // creation second, including ties since existing rows store whole seconds.
    fn matching_id(&self, tool: &str, input: &Value, created: u64) -> Option<&str> {
        let mut ids = self.uses.iter().filter_map(|(id, (name, args, started))| {
            let ended_before = self
                .results
                .get(id)
                .is_some_and(|end| *end != 0 && end / 1000 < created);
            (name == tool && args == input && started / 1000 <= created && !ended_before)
                .then_some(id.as_str())
        });
        let id = ids.next()?;
        ids.next().is_none().then_some(id)
    }

    pub(super) fn open_id(&self, tool: &str, input: &Value, created: u64) -> Option<&str> {
        self.matching_id(tool, input, created)
            .filter(|id| !self.results.contains_key(*id))
    }

    pub(super) fn resolved(&self, request: &Value, created: u64) -> bool {
        if let Some(id) = request["toolUseId"].as_str() {
            return self.results.contains_key(id);
        }
        // A hook started by an older build has no saved ID. For these cards
        // require a result strictly later than the entire creation second, so
        // an earlier, already completed call cannot dismiss a current card.
        let Some(tool) = request["tool"].as_str() else {
            return false;
        };
        self.matching_id(tool, &request["input"], created)
            .and_then(|id| self.results.get(id))
            .is_some_and(|ms| ms / 1000 > created)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn desktop_result_settles_its_card_and_preserves_ambiguous_repeats() {
        let mut calls = Calls::default();
        let input = json!({"title":"QA"});
        let mut card = json!({"tool":"start_session","input":input});
        calls.uses.insert(
            "first".into(),
            ("start_session".into(), input.clone(), 99950),
        );
        assert_eq!(calls.open_id("start_session", &input, 100), Some("first"));
        assert!(!calls.resolved(&card, 100));
        calls.results.insert("unrelated".into(), 120000);
        assert!(!calls.resolved(&card, 100));
        calls.results.insert("first".into(), 120000);
        assert!(calls.resolved(&card, 100));
        assert!(!calls.resolved(&card, 125));
        calls
            .uses
            .insert("second".into(), ("start_session".into(), input, 100010));
        assert!(!calls.resolved(&card, 100));
        card["toolUseId"] = json!("second");
        assert!(!calls.resolved(&card, 100));
        card["toolUseId"] = json!("first");
        assert!(calls.resolved(&card, 100));
    }
}
