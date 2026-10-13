//! Native task control retains model/settings and uses the existing durable dispatch ledger.
use crate::{attachments, native, request_id, Bridge};
use serde_json::{json, Value};

impl Bridge {
    pub(crate) fn project_events(&self,batch:&mut Value) {
        for entry in batch["events"].as_array_mut().into_iter().flatten() {
            let message=&mut entry["message"];
            if matches!(message["method"].as_str(),Some("item/started"|"item/completed")) {
                let item=&mut message["params"]["item"];
                super::output::project(&self.attachments,item);
                if item["type"]=="userMessage" {item["content"]=json!(self.attachments.project_content(&item["content"]));}
            }
        }
    }

    pub(crate) fn native_subscribe(&self, thread: &str) -> Result<Value,String> {
        let mut result=native::request(&self.home,"thread/resume",json!({"threadId":thread,"excludeTurns":true,
            "initialTurnsPage":{"limit":30,"itemsView":"full","sortDirection":"desc"}}))?;
        result["thread"]["historyCursor"]=result["initialTurnsPage"]["nextCursor"].as_str().map(|c|json!(format!("engine:{c}"))).unwrap_or(Value::Null);
        if let Some(turns)=result["initialTurnsPage"]["data"].as_array() {
            let mut turns=turns.clone(); turns.reverse(); result["thread"]["turns"]=json!(turns);
        }
        result["thread"]["streamPosition"]=result["_clawkitStream"].clone();
        let pending=native::read_events(&self.home,json!({}))?;
        result["pendingHumanRequest"]=json!(pending["pendingRequests"].as_array().into_iter().flatten().any(|r|r["params"]["threadId"]==thread));
        self.native_reconcile(thread,&result["thread"])?;
        result["queue"]=json!(self.queue.list(thread)?);
        self.project_native_items(&mut result["thread"]);
        Ok(result)
    }

    fn project_native_items(&self, thread:&mut Value) {
        if let Some(turns)=thread["turns"].as_array_mut() {
            for item in turns.iter_mut().flat_map(|t|t["items"].as_array_mut().into_iter().flatten()) {
                super::output::project(&self.attachments,item);
                if item["type"]=="userMessage" {item["content"]=json!(self.attachments.project_content(&item["content"]));}
            }
        }
    }

    pub(crate) fn native_history(&self,thread:&str,cursor:&str)->Result<Value,String> {
        if cursor.len()>16000 {return Err("历史游标无效".into());}
        let page=native::request(&self.home,"thread/turns/list",json!({"threadId":thread,"cursor":cursor,"limit":30,"itemsView":"full","sortDirection":"desc"}))?;
        let mut turns=page["data"].as_array().ok_or("历史响应格式不兼容")?.clone(); turns.reverse();
        let next=page["nextCursor"].as_str().map(|c|format!("engine:{c}"));
        let mut thread=json!({"id":thread,"turns":turns,"historyCursor":next});
        self.project_native_items(&mut thread);
        Ok(json!({"thread":thread,"nextCursor":next}))
    }

    pub(crate) fn native_reconcile(&self, thread: &str, state: &Value) -> Result<(),String> {
        for id in self.ledger.unresolved(thread)? {
            if state["turns"].as_array().into_iter().flatten()
                .flat_map(|v|v["items"].as_array().into_iter().flatten())
                .any(|v|v["type"]=="userMessage" && (v["clientId"]==id || v["id"]==id)) {
                self.ledger.mark(&id,"accepted")?;
            }
        }
        Ok(())
    }

    pub(crate) fn native_start(&self, params:&Value)->Result<Value,String> { self.native_send(params,true) }

    pub(crate) fn native_send(&self, params:&Value, steer:bool)->Result<Value,String> {
        let object=params.as_object().ok_or("发送参数无效")?;
        if object.keys().any(|k|!["threadId","requestId","text","images","files"].contains(&k.as_str())) {
            return Err("不能覆盖原任务的权限或模型配置".into());
        }
        let thread=self.thread(params)?;
        let id=request_id(params)?;
        let message=attachments::Message::parse(params)?;
        let identity=message.identity();
        if let Some(receipt)=self.ledger.existing(id,thread,&identity)? {return Ok(receipt);}
        let current=self.native_subscribe(thread)?;
        let pending=native::read_events(&self.home,json!({}))?;
        if pending["pendingRequests"].as_array().into_iter().flatten().any(|r|r["params"]["threadId"]==thread) {
            return Err("请先处理此任务的审批或问题".into());
        }
        let active=current["thread"]["turns"].as_array().into_iter().flatten().rev().find(|t|t["status"]=="inProgress");
        if active.is_some() && !steer {return Err("任务仍在执行，继续等待队列".into());}
        let input=self.attachments.input(&message)?;
        if let Some(receipt)=self.ledger.begin(id,thread,&identity)? {return Ok(receipt);}
        if !self.enabled.load(std::sync::atomic::Ordering::SeqCst) {
            self.ledger.mark(id,"rejected")?;return self.ledger.read(id,thread);
        }
        let (method,args)=match active {
            Some(turn)=>("turn/steer",json!({"threadId":thread,"expectedTurnId":turn["id"],"input":input,"clientUserMessageId":id})),
            None=>("turn/start",json!({"threadId":thread,"input":input,"clientUserMessageId":id})),
        };
        let status=if native::request(&self.home,method,args).is_ok(){"accepted"}else{"unknown"};
        self.ledger.mark(id,status)?;
        self.ledger.read(id,thread)
    }
}
