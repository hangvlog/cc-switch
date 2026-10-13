//! Convert explicit human decisions using the original engine request's scope.
use serde_json::{json, Value};

pub(crate) fn remote_form(request: &Value) -> bool {
    matches!(request["params"]["mode"].as_str(), Some("form" | "openai/form" | "openaiForm"))
}

pub(crate) fn build(request: &Value, params: &Value) -> Result<Value, String> {
    let decision=params["decision"].as_str().unwrap_or("");
    match request["method"].as_str().unwrap_or("") {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            if let Some(allowed)=request["params"]["availableDecisions"].as_array() {
                // The phone identifies an offered choice; its policy content always comes from the engine.
                let choice=allowed.iter().find(|v|v.as_str()==Some(decision) || v.as_object().is_some_and(|o|o.len()==1 && o.contains_key(decision)))
                    .ok_or("当前请求不允许此决定")?;
                return Ok(json!({"decision":choice}));
            }
            if !["accept","acceptForSession","decline","cancel"].contains(&decision) {return Err("审批决定无效".into());}
            Ok(json!({"decision":decision}))
        }
        "execCommandApproval" | "applyPatchApproval" => {
            let decision=match decision {"accept"=>"approved","acceptForSession"=>"approved_for_session","decline"=>"denied","cancel"=>"abort",_=>return Err("审批决定无效".into())};
            Ok(json!({"decision":decision}))
        }
        "item/permissions/requestApproval" => {
            if !["accept","acceptForSession","decline"].contains(&decision) {return Err("权限决定无效".into());}
            // Never accept a permission profile provided by a remote caller.
            let permissions=if decision=="decline" {json!({})} else {request["params"]["permissions"].clone()};
            if !permissions.is_object() {return Err("权限请求格式不兼容".into());}
            Ok(json!({"permissions":permissions,"scope":if decision=="acceptForSession" {"session"} else {"turn"}}))
        }
        "item/tool/requestUserInput" => {
            let answers=params["answers"].as_object().ok_or("缺少问题回答")?;
            let questions=request["params"]["questions"].as_array().ok_or("问题格式不兼容")?;
            if answers.len()!=questions.len() {return Err("请回答每个问题".into());}
            for question in questions {
                let id=question["id"].as_str().ok_or("问题缺少编号")?;
                let values=answers.get(id).and_then(|v|v["answers"].as_array()).ok_or("回答格式无效")?;
                if values.is_empty() || values.iter().any(|v|v.as_str().is_none_or(|s|s.len()>16000)) {return Err("回答内容无效".into());}
            }
            Ok(json!({"answers":answers}))
        }
        "mcpServer/elicitation/request" => {
            if !["accept","decline","cancel"].contains(&decision) {return Err("输入请求决定无效".into());}
            // URL-mode completion belongs to the original auth UI; never emulate completion.
            if !remote_form(request) {return Err("请在电脑完成此身份验证".into());}
            let content=params.get("content").cloned().unwrap_or(Value::Null);
            if decision=="accept" {
                let object=content.as_object().ok_or("请填写表单")?;
                let schema=&request["params"]["requestedSchema"];
                let properties=schema["properties"].as_object().ok_or("表单格式不兼容")?;
                if object.keys().any(|k|!properties.contains_key(k)) {return Err("表单包含未知字段".into());}
                for key in schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    if !object.contains_key(key) {return Err("请填写所有必填字段".into());}
                }
                for (key,value) in object {
                    let spec=&properties[key];
                    let valid=match spec["type"].as_str() {
                        Some("string")=>value.is_string(), Some("number")=>value.is_number(),
                        Some("integer")=>value.is_i64()||value.is_u64(), Some("boolean")=>value.is_boolean(),
                        Some("array")=>value.as_array().is_some_and(|items|items.len()<=100 && items.iter().all(Value::is_string)),
                        _=>false,
                    };
                    if !valid {return Err("表单字段类型不兼容".into());}
                    validate_field(spec,value)?;
                }
            }
            Ok(json!({"action":decision,"content":content}))
        }
        _=>Err("此请求必须在电脑处理".into()),
    }
}

fn validate_field(spec:&Value,value:&Value)->Result<(),String> {
    if let Some(options)=spec["enum"].as_array() {if !options.contains(value){return Err("表单选项无效".into());}}
    if let Some(options)=spec["oneOf"].as_array().or_else(||spec["anyOf"].as_array()) {
        if !options.iter().any(|o|o.get("const")==Some(value)){return Err("表单选项无效".into());}
    }
    if let Some(number)=value.as_f64() {
        if spec["minimum"].as_f64().is_some_and(|n|number<n) || spec["maximum"].as_f64().is_some_and(|n|number>n) {return Err("表单数字超出范围".into());}
    }
    if let Some(text)=value.as_str() {
        let len=text.chars().count() as u64;
        if spec["minLength"].as_u64().is_some_and(|n|len<n) || spec["maxLength"].as_u64().is_some_and(|n|len>n) {return Err("表单文字长度无效".into());}
    }
    if let Some(items)=value.as_array() {
        if spec["minItems"].as_u64().is_some_and(|n|items.len()<(n as usize)) || spec["maxItems"].as_u64().is_some_and(|n|items.len()>(n as usize)) {return Err("表单选项数量无效".into());}
        if spec["uniqueItems"]==true && items.iter().enumerate().any(|(i,v)|items[..i].contains(v)) {return Err("表单选项不能重复".into());}
        for item in items {validate_field(&spec["items"],item)?;}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offered_decisions_questions_and_forms_are_validated() {
        let request=json!({"method":"item/commandExecution/requestApproval","params":{"availableDecisions":["cancel",{"acceptWithExecpolicyAmendment":{"execpolicy_amendment":["git","status"]}}]}});
        let result=build(&request,&json!({"decision":"acceptWithExecpolicyAmendment","execpolicy_amendment":["rm"]})).unwrap();
        assert_eq!(result["decision"]["acceptWithExecpolicyAmendment"]["execpolicy_amendment"][0],"git");
        assert!(build(&request,&json!({"decision":"accept"})).is_err());
        let question=json!({"method":"item/tool/requestUserInput","params":{"questions":[{"id":"q"}]}});
        assert!(build(&question,&json!({"answers":{"q":{"answers":["yes"]}}})).is_ok());
        assert!(build(&question,&json!({"answers":{"wrong":{"answers":["yes"]}}})).is_err());
        let form=json!({"method":"mcpServer/elicitation/request","params":{"mode":"form","requestedSchema":{"properties":{"n":{"type":"integer","minimum":2},"pick":{"type":"string","enum":["a","b"]}},"required":["n"]}}});
        assert!(build(&form,&json!({"decision":"accept","content":{"n":2,"pick":"a"}})).is_ok());
        assert!(build(&form,&json!({"decision":"accept","content":{"n":1}})).is_err());
        assert!(build(&form,&json!({"decision":"accept","content":{"n":2,"pick":"z"}})).is_err());
    }
    #[test]
    fn permissions_are_taken_only_from_pending_request() {
        let request=json!({"method":"item/permissions/requestApproval","params":{"permissions":{"network":{"enabled":true}}}});
        let result=build(&request,&json!({"decision":"accept","permissions":{"fileSystem":{"write":["/"]}}})).unwrap();
        assert!(result["permissions"].get("fileSystem").is_none());
        assert_eq!(result["scope"],"turn");
        assert_eq!(build(&request,&json!({"decision":"decline"})).unwrap()["permissions"],json!({}));
    }
}
