use super::pipe::Pipe;
use serde_json::{json,Value};
use std::collections::HashMap;

fn properties(pipe:&mut Pipe,session:&str,id:&str)->Result<Value,String>{
    pipe.call("Runtime.getProperties",json!({"objectId":id,"ownProperties":true}),Some(session))
}
fn module(pipe:&mut Pipe,session:&str,expression:&str,required:&str)->Result<HashMap<String,String>,String>{
    let result=pipe.call("Runtime.evaluate",json!({"expression":expression,"awaitPromise":true}),Some(session))?;
    let function=result["result"]["objectId"].as_str().ok_or("官方窗口导出不兼容")?;
    let p=properties(pipe,session,function)?;
    let scopes=p["internalProperties"].as_array().and_then(|p|p.iter().find(|p|p["name"]=="[[Scopes]]"))
        .and_then(|p|p["value"]["objectId"].as_str()).ok_or("官方窗口闭包不可读")?;
    let scopes=properties(pipe,session,scopes)?;
    for scope in scopes["result"].as_array().into_iter().flatten(){
        if scope["value"]["description"]!="Module" {continue}
        let Some(id)=scope["value"]["objectId"].as_str() else{continue};
        let members=properties(pipe,session,id)?;
        let refs:HashMap<_,_>=members["result"].as_array().into_iter().flatten().filter_map(|v|Some((v["name"].as_str()?.to_owned(),v["value"]["objectId"].as_str()?.to_owned()))).collect();
        if refs.contains_key(required){return Ok(refs)}
    }
    Err("官方窗口版本不兼容".into())
}
pub(super) fn install(pipe:&mut Pipe,session:&str)->Result<(),String>{
    // Hash the resources from the actual running renderer before selecting aliases.
    let hashes=pipe.evaluate(session,r#"(async()=>{const hash=async name=>{const r=await fetch('app://-/assets/'+name);if(!r.ok)throw Error('asset unavailable');return Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',await r.arrayBuffer())),x=>x.toString(16).padStart(2,'0')).join('')};return [await hash('app-initial-6350d04672d0.js'),await hash('app-shared-8bd9d0399038.js')]})()"#)?;
    if hashes!=json!(["604b5d2de4fbb41f3d0dc5b3662129c558e1c7b8aac8cc87225286f58dcfaf8f","607ad0866dd25ed3764711b4467a9b60708353a599485b5249ff4cbe0e870245"]){return Err("当前官方版本尚未适配活动同步".into())}
    let refs=module(pipe,session,"(async()=>(await import('app://-/assets/app-initial-6350d04672d0.js')).Oen)()","nj")?;
    let resolve=pipe.call("Runtime.callFunctionOn",json!({"objectId":refs["nj"],"functionDeclaration":"function(){return this.resolve}"}),Some(session))?;
    let resolve_id=resolve["result"]["objectId"].as_str().ok_or("状态函数不可用")?;
    // Transfer just the resolve function to a temporary name so the same scoped
    // inspection can find the official scope factory. No bundle is rewritten.
    pipe.call("Runtime.callFunctionOn",json!({"objectId":resolve_id,"functionDeclaration":"function(){globalThis.__clawkitScopeResolve=this}"}),Some(session))?;
    let shared=module(pipe,session,"globalThis.__clawkitScopeResolve","Nk")?;
    let mut args=vec![json!({"objectId":shared["Nk"]})];
    for name in ["nj","pei","gei","mei","zH","nti","ati","q","Qer","Vei"] {
        args.push(json!({"objectId":refs.get(name).ok_or("官方活动函数缺失")?}));
    }
    let result=pipe.call("Runtime.callFunctionOn",json!({"objectId":shared["Nk"],"functionDeclaration":include_str!("activity.js"),"arguments":args,"returnByValue":true}),Some(session))?;
    pipe.evaluate(session,"delete globalThis.__clawkitScopeResolve")?;
    if result["result"]["value"]!=true{return Err("官方活动适配未就绪".into())}
    Ok(())
}
