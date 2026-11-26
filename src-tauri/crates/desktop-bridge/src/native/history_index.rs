//! Read only the user-visible rows needed from Codex's paginated history projection.
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{json, Value};
use std::path::Path;

pub(crate) fn open(home: &Path) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(
        home.join("thread_history_1.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
}

pub(super) fn contains_message(home: &Path, thread: &str, client: &str) -> bool {
    open(home)
        .and_then(|db| {
            db.query_row(
        "SELECT EXISTS(SELECT 1 FROM thread_items WHERE thread_id=?1 AND item_type='userMessage'
         AND (item_id=?2 OR json_extract(item_json,'$.clientId')=?2))",
        [thread, client], |r| r.get(0))
        })
        .unwrap_or(false)
}

// Keep the real turn IDs for forks and asynchronous questions. Latest text is
// selected across turns, so a long-running tool cannot bury the last question.
pub(crate) fn latest(
    home: &Path,
    thread: &str,
    active: Option<&str>,
) -> rusqlite::Result<(Vec<Value>, Vec<Value>)> {
    let db = open(home)?;
    let mut turns = Vec::new();
    for kind in ["userMessage", "agentMessage"] {
        let row = db.query_row(
            "SELECT i.turn_id,t.status,i.item_json FROM thread_items i JOIN thread_turns t
             ON i.turn_id=t.turn_id AND i.thread_id=t.thread_id
             WHERE i.thread_id=?1 AND i.item_type=?2 AND length(i.item_json)<524288
             AND (?2!='agentMessage' OR COALESCE(json_extract(i.item_json,'$.phase'),'')!='analysis')
             AND (?2!='agentMessage' OR i.rollout_ordinal>COALESCE((SELECT max(rollout_ordinal) FROM thread_items
             WHERE thread_id=?1 AND item_type='userMessage'),-1))
             ORDER BY i.rollout_ordinal DESC LIMIT 1", [thread,kind],
             |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?, r.get::<_,String>(2)?))).optional()?;
        if let Some((id, status, raw)) = row {
            if let Ok(item) = serde_json::from_str::<Value>(&raw) {
                if let Some(last) = turns.last_mut().filter(|t: &&mut Value| t["id"] == id) {
                    last["items"].as_array_mut().unwrap().push(item);
                } else {
                    turns.push(json!({"id":id,"status":status,"items":[item]}));
                }
            }
        }
    }
    let mut questions = vec![];
    if let Some(turn) = active {
        let mut query = db.prepare("SELECT item_json FROM thread_items WHERE thread_id=?1 AND turn_id=?2
          AND item_type IN ('userMessage','agentMessage') AND length(item_json)<524288 AND
          (json_array_length(item_json,'$.questions')>0 OR instr(item_json,'<send_user_message_question_reply>')>0)
          ORDER BY rollout_ordinal")?;
        let items = query
            .query_map([thread, turn], |r| r.get::<_, String>(0))?
            .filter_map(|s| s.ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()))
            .collect::<Vec<_>>();
        questions = crate::clarification::pending(
            &json!({"id":thread,"turns":[{"turnId":turn,"status":"inProgress","items":items}]}),
        );
    }
    Ok((turns, questions))
}

pub(crate) fn anchor(db: &Connection, thread: &str, item: &str, offset: u64) -> Option<String> {
    db.query_row("SELECT turn_id FROM thread_items WHERE thread_id=?1 AND item_id=?2",
        [thread,item],|r|r.get(0)).optional().ok()?.or_else(||db.query_row(
        "SELECT turn_id FROM thread_turns WHERE thread_id=?1 AND rollout_byte_offset<=?2
         AND (rollout_end_byte_offset IS NULL OR rollout_end_byte_offset>?2) ORDER BY rollout_ordinal DESC LIMIT 1",
        rusqlite::params![thread,offset],|r|r.get(0)).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_async_questions_keep_their_turn_and_disappear_after_reply() {
        let root = tempfile::tempdir().unwrap();
        let db = Connection::open(root.path().join("thread_history_1.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE thread_items(thread_id,turn_id,item_id,item_type,item_json,rollout_ordinal);
            CREATE TABLE thread_turns(thread_id,turn_id,status);
            INSERT INTO thread_turns VALUES ('t','a','inProgress')").unwrap();
        let ask = json!({"id":"q","type":"agentMessage","text":"hello","questions":[{"title":"选择？","options":["A","B"]}]});
        db.execute(
            "INSERT INTO thread_items VALUES ('t','a','q','agentMessage',?1,1)",
            [ask.to_string()],
        )
        .unwrap();
        let (_, questions) = latest(root.path(), "t", Some("a")).unwrap();
        assert_eq!(questions[0]["params"]["questions"][0]["question"], "选择？");
        let id = &questions[0]["params"]["questions"][0]["id"];
        let answer = json!({"id":"u","type":"userMessage","clientId":"receipt","content":[{"type":"text",
            "text":format!("<send_user_message_question_reply>\n{}\n</send_user_message_question_reply>",json!([{"questionItemId":id,"question":"选择？","answer":"C"}]))}]});
        db.execute(
            "INSERT INTO thread_items VALUES ('t','a','u','userMessage',?1,2)",
            [answer.to_string()],
        )
        .unwrap();
        assert!(latest(root.path(), "t", Some("a")).unwrap().1.is_empty());
        assert!(latest(root.path(), "t", None).unwrap().1.is_empty());
        assert!(contains_message(root.path(), "t", "receipt"));
        assert!(!contains_message(root.path(), "other", "receipt"));
    }
}
