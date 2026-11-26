//! Native task control retains model/settings and uses the existing durable dispatch ledger.
use crate::{attachments, native, request_id, Bridge};
use serde_json::{json, Value};

impl Bridge {
    pub(crate) fn project_events(&self,batch:&mut Value) {
        super::output::project_batch(&self.attachments,batch);
    }

    pub(crate) fn native_subscribe(&self, thread: &str) -> Result<Value,String> {
        let mut result = native::request(&self.home, "thread/resume", json!({
            "threadId":thread, "excludeTurns":true,
            "initialTurnsPage":{"limit":1,"itemsView":"notLoaded","sortDirection":"desc"}
        }))?;
        if result["thread"]["id"] != thread { return Err("电脑返回了不同的对话".into()); }
        let recent = result.as_object_mut().and_then(|r|r.remove("initialTurnsPage"))
            .and_then(|mut p|p["data"].take().as_array().cloned()).ok_or("电脑缺少当前轮次状态")?;
        result["thread"]["turns"] = json!(recent);
        result["thread"]["streamPosition"] = result["_clawkitStream"].clone();
        let pending = native::read_events(&self.home, json!({}))?;
        let flags = &result["thread"]["status"]["activeFlags"];
        result["pendingHumanRequest"] = json!(pending["pendingRequests"].as_array().into_iter().flatten()
            .any(|r|r["params"]["threadId"]==thread) || flags.as_array().is_some_and(|a|!a.is_empty()));
        self.native_reconcile(thread,&result["thread"])?;
        result["queue"]=json!(self.queue.list(thread)?);
        Ok(result)
    }

    pub(crate) fn native_subscribe_page(&self, thread: &str, latest_only: bool) -> Result<Value,String> {
        let mut result = self.native_subscribe(thread)?;
        let active = result["thread"]["turns"][0].clone();
        let active_id = active["id"].as_str().filter(|_|active["status"]=="inProgress");
        let (mut turns, questions) = super::history_index::latest(&self.home, thread, active_id).unwrap_or_default();
        if turns.is_empty() || !latest_only {
            if let Ok(saved) = crate::saved_history::read_page(&self.home,thread,None,false,&self.attachments,if latest_only {1} else {20}) {
                turns = saved["thread"]["turns"].as_array().cloned().unwrap_or(turns);
                result["thread"]["historyCursor"] = saved["thread"]["historyCursor"].clone();
            }
        } else {
            let mut visible = json!({"turns":turns});
            self.project_native_items(&mut visible);
            turns = std::mem::take(visible["turns"].as_array_mut().unwrap());
        }
        for turn in &mut turns {
            if turn["id"] == active["id"] { turn["status"] = active["status"].clone(); }
        }
        if active_id.is_some() && !turns.iter().any(|t| t["id"]==active["id"]) { turns.push(active); }
        result["thread"]["turns"]=json!(turns);
        result["thread"]["asyncQuestions"]=json!(questions);
        if latest_only { crate::latest_message::compact(&mut result["thread"]); }
        Ok(result)
    }

    fn project_native_items(&self, thread:&mut Value) {
        super::output::project_items(&self.attachments,thread);
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
            if super::history_index::contains_message(&self.home, thread, &id) || state["turns"].as_array().into_iter().flatten()
                .flat_map(|v|v["items"].as_array().into_iter().flatten())
                .any(|v|v["type"]=="userMessage" && (v["clientId"]==id || v["id"]==id)) {
                self.ledger.mark(&id,"accepted")?;
            }
        }
        Ok(())
    }

    pub(crate) fn native_answer(&self,params:&Value,text:&str,turn:&str)->Result<Value,String> {
        let args=json!({"threadId":params["threadId"],"requestId":params["requestId"],"text":text});
        self.send_checked(&args,true,Some(turn))
    }

    pub(crate) fn native_start(&self, params:&Value)->Result<Value,String> { self.native_send(params,true) }

    pub(crate) fn native_send(&self, params:&Value, steer:bool)->Result<Value,String> {
        self.send_checked(params,steer,None)
    }

    fn send_checked(&self, params:&Value, steer:bool, expected:Option<&str>)->Result<Value,String> {
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
        if current["pendingHumanRequest"] == true {return Err("请先处理此任务的审批或问题".into());}
        let active=current["thread"]["turns"].as_array().into_iter().flatten().rev().find(|t|t["status"]=="inProgress");
        if current["thread"]["status"]["type"]=="active" && active.is_none() { return Err("电脑正在切换执行轮次，请刷新后发送".into()); }
        if expected.is_some_and(|id|active.and_then(|t|t["id"].as_str())!=Some(id)) {return Err("问题所属轮次已结束，请刷新".into());}
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
        let status=match native::request_outcome(&self.home,method,args) {
            Ok(value) if value["turn"]["id"].is_string() || value["turnId"].is_string() => "accepted",
            Err(error) if error.definite => { self.ledger.mark(id,"rejected")?;return Ok(json!({"status":"rejected","reason":error.message})); }
            _ => "unknown",
        };
        self.ledger.mark(id,status)?;
        self.ledger.read(id,thread)
    }
}
