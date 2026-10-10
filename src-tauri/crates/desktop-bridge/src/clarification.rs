use crate::{history, ipc::IpcClient, Bridge};
use serde_json::{json, Value};
use std::collections::HashSet;

pub fn pending(state: &Value) -> Vec<Value> {
    let mut result: Vec<Value> = state["requests"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["method"] == "item/tool/requestUserInput")
        .map(|r| json!({"requestId":r["id"],"method":r["method"],"params":r["params"]}))
        .collect();
    // Asynchronous questions are attached to agent messages, not RPC requests.
    let all = history::turns(state);
    let all_items: Vec<_> = all
        .iter()
        .flat_map(|t| t["items"].as_array().into_iter().flatten())
        .collect();
    let answered: HashSet<String> = all_items
        .iter()
        .filter(|i| {
            i["type"] == "userMessage"
                || i["type"] == "steeringUserMessage" && i["status"] == "accepted"
        })
        .flat_map(|i| {
            i.get("content")
                .or_else(|| i.get("input"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|p| p["text"].as_str())
        .filter_map(|s| {
            s.trim()
                .strip_prefix("<send_user_message_question_reply>")?
                .strip_suffix("</send_user_message_question_reply>")
        })
        .filter_map(|s| serde_json::from_str::<Vec<Value>>(s.trim()).ok())
        .flatten()
        .filter_map(|v| v["questionItemId"].as_str().map(str::to_owned))
        .collect();
    for turn in all {
        let items: Vec<_> = turn["items"].as_array().into_iter().flatten().collect();
        for item in items.iter().filter(|i| i["type"] == "agentMessage") {
            let questions:Vec<_>=item["questions"].as_array().into_iter().flatten().enumerate().filter_map(|(n,q)|{
                let id=json!(["request_user_input_async",item["id"],n]).to_string();
                if answered.contains(&id){return None;}
                Some(json!({"id":id,"header":"补充信息","question":q.get("question").or_else(||q.get("title")).unwrap_or(&Value::Null),"options":q["options"],"isOther":true}))
            }).collect();
            if !questions.is_empty() {
                result.push(json!({"requestId":format!("async:{}",item["id"].as_str().unwrap_or("")),"method":"clawkit/asyncQuestions","params":{"threadId":state["id"],"turnId":turn["turnId"],"itemId":item["id"],"isBlocking":false,"questions":questions}}));
            }
        }
    }
    result
}
fn validate(request: &Value, answers: &Value) -> Result<(), String> {
    let questions = request["params"]["questions"]
        .as_array()
        .ok_or("问题格式不兼容")?;
    let map = answers
        .as_object()
        .filter(|m| m.len() == questions.len() && !m.is_empty())
        .ok_or("请回答所有问题")?;
    for q in questions {
        let id = q["id"].as_str().ok_or("问题缺少编号")?;
        let values = map
            .get(id)
            .and_then(|v| v["answers"].as_array())
            .filter(|a| a.len() == 1)
            .ok_or("每个问题需要一个回答")?;
        let answer = values[0]
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.len() <= 8000)
            .ok_or("回答不能为空或过长")?;
        let options = q["options"].as_array();
        if q["isOther"] == false
            && options.is_some_and(|o| !o.is_empty() && !o.iter().any(|o| o["label"] == answer))
        {
            return Err("请选择有效选项".into());
        }
    }
    Ok(())
}
fn confirmed(state: &Value, request: &Value, answers: &Value) -> bool {
    history::turns(state)
        .iter()
        .flat_map(|t| t["items"].as_array().into_iter().flatten())
        .any(|i| {
            i["type"] == "userInputResponse"
                && i["requestId"] == request["requestId"]
                && i["completed"] == true
                && answers
                    .as_object()
                    .is_some_and(|a| a.iter().all(|(k, v)| i["answers"][k] == v["answers"]))
        })
}
impl Bridge {
    pub(crate) fn answer(&self, params: &Value) -> Result<Value, String> {
        let thread = self.thread(params)?;
        if !self.enabled.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("远程连接已关闭".into());
        }
        let mut client = IpcClient::connect(&self.home)?;
        let snapshot = client.snapshot(thread, None)?;
        if let Some(reason) = history::conversation_block_reason(&snapshot.state) {
            return Err(reason.into());
        }
        let requests = pending(&snapshot.state);
        let request = requests
            .iter()
            .find(|r| r["requestId"] == params["questionRequestId"])
            .ok_or("问题已更新或已回答，请刷新对话")?;
        let answers = &params["answers"];
        validate(request, answers)?;
        if request["method"] == "clawkit/asyncQuestions" {
            let id = crate::request_id(params)?;
            let replies:Vec<_>=request["params"]["questions"].as_array().unwrap().iter().map(|q|json!({"questionItemId":q["id"],"question":q.get("question").or_else(||q.get("title")).unwrap_or(&Value::Null),"answer":answers[q["id"].as_str().unwrap()]["answers"][0]})).collect();
            let text = format!(
                "<send_user_message_question_reply>\n{}\n</send_user_message_question_reply>",
                json!(replies)
            );
            if let Some(old) = self.ledger.existing(id, thread, &text)? {
                return Ok(old);
            }
            if history::send_block_reason(&snapshot.state).is_none() {
                return self.start(&json!({"threadId":thread,"requestId":id,"text":text}));
            }
            if !snapshot.state["requests"]
                .as_array()
                .is_some_and(Vec::is_empty)
                || !history::turns(&snapshot.state)
                    .last()
                    .is_some_and(|t| t["status"] == "inProgress")
            {
                return Err("请先处理当前审批或阻塞问题".into());
            }
            self.ledger.begin(id, thread, &text)?;
            let result=client.request("thread-follower-steer-turn",json!({"conversationId":thread,"clientUserMessageId":id,"restoreMessage":{"id":id,"cwd":snapshot.state["cwd"],"context":{"text":text,"commentAttachments":[],"fileAttachments":[],"imageAttachments":[],"addedFiles":[],"workspaceRoots":[snapshot.state["cwd"]]}},"input":[{"type":"text","text":text,"text_elements":[]}]}),1,Some(&snapshot.owner));
            // Acceptance requires the original owner's accepted user message, not an IPC ack.
            self.ledger.mark(
                id,
                if result
                    .as_ref()
                    .err()
                    .is_some_and(|e| e == "Codex IPC: request-version-mismatch")
                {
                    "rejected"
                } else {
                    "unknown"
                },
            )?;
            for delay in [0, 200, 500] {
                std::thread::sleep(std::time::Duration::from_millis(delay));
                if let Ok(s) =
                    IpcClient::connect(&self.home).and_then(|mut c| c.snapshot(thread, None))
                {
                    self.reconcile(thread, &s.state)?;
                    if self.ledger.read(id, thread)?["status"] == "accepted" {
                        break;
                    }
                }
            }
            return self.ledger.read(id, thread);
        }
        let _=client.request("thread-follower-submit-user-input",json!({"conversationId":thread,"requestId":request["requestId"],"response":{"answers":answers}}),1,Some(&snapshot.owner));
        let fresh = IpcClient::connect(&self.home)?.snapshot(thread, Some(&snapshot.owner))?;
        if confirmed(&fresh.state, request, answers) {
            Ok(json!({"status":"accepted"}))
        } else {
            Err("尚未确认回答，请刷新核对；输入已保留".into())
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projects_sync_and_async_questions_and_filters_answers() {
        let mut s = json!({"id":"t","requests":[{"id":9,"method":"item/tool/requestUserInput","params":{"questions":[{"id":"q","isOther":false,"options":[{"label":"A"}]}]}}],"turns":[{"turnId":"turn","items":[{"id":"call","type":"agentMessage","questions":[{"question":"Where?","options":[{"label":"Here"}]}]}]}]});
        let p = pending(&s);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0]["requestId"], 9);
        assert!(validate(&p[0], &json!({"q":{"answers":["invalid"]}})).is_err());
        assert!(validate(&p[0], &json!({"q":{"answers":["A"]}})).is_ok());
        let reply = json!([{"questionItemId":p[1]["params"]["questions"][0]["id"],"question":"Where?","answer":"Here"}]);
        s["turns"][0]["items"].as_array_mut().unwrap().push(json!({"type":"userMessage","content":[{"type":"text","text":format!("<send_user_message_question_reply>\n{reply}\n</send_user_message_question_reply>")}]}));
        assert_eq!(pending(&s).len(), 1);
    }
}
