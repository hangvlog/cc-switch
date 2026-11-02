//! The desktop's own sidebar layout (sections, group assignments, pins), read
//! from `claude_desktop_config.json` so the phone lists sessions the same way.
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

#[derive(Default)]
pub struct Sidebar {
    pub sections: Vec<Value>,
    pub assignments: HashMap<String, String>,
    pub pinned: HashSet<String>,
}

/// Prefs are keyed by `<account>/<org>`; the phone sees one desktop, so the
/// scope that actually holds Code sessions wins.
fn scope<'a>(prefs: &'a Value, key: &str) -> Option<&'a Value> {
    let scopes = prefs[key].as_object()?;
    scopes.values().max_by_key(|v| v["assignments"].as_object().map_or(0, |a| a.len()) + v["sections"].as_array().map_or(0, |s| s.len()))
}

pub fn read(app_data: Option<&Path>) -> Sidebar {
    let Some(prefs) = app_data
        .and_then(|dir| fs::read(dir.join("claude_desktop_config.json")).ok())
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .map(|config| config["preferences"]["epitaxyPrefs"].clone())
    else {
        return Sidebar::default();
    };
    let assignments = scope(&prefs, "dframe-group-scopes")
        .and_then(|s| s["assignments"].as_object())
        .map(|a| a.iter().filter_map(|(k, v)| Some((k.strip_prefix("code:")?.to_string(), v.as_str()?.to_string()))).collect())
        .unwrap_or_default();
    let mut sections: Vec<&Value> = scope(&prefs, "dframe-code-sections")
        .and_then(|s| s["sections"].as_array())
        .map(|list| list.iter().collect())
        .unwrap_or_default();
    sections.sort_by_key(|s| s["order"].as_i64().unwrap_or(i64::MAX));
    let sections = sections
        .into_iter()
        .filter(|s| matches!(s["kind"].as_str(), Some("pinned" | "manual" | "sessions")) && s["shown"] != false && s["show"] != false)
        .filter_map(|s| Some(json!({"id":s["id"].as_str()?,"kind":s["kind"],"name":s["name"].as_str().unwrap_or(""),"collapsed":s["collapsed"] == true})))
        .collect();
    let pinned = prefs["starred-local-code-sessions"]
        .as_array()
        .map(|ids| ids.iter().filter_map(|v| v.as_str().map(|id| id.trim_start_matches("code:").to_string())).collect())
        .unwrap_or_default();
    Sidebar { sections, assignments, pinned }
}
