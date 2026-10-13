//! Narrow, same-user activity/read-state service attached at official app launch.
//! Uses inherited pipes; no CDP port, global config change, or application patch.
mod pipe;
mod bind;
use serde_json::{json,Value};
use std::{io::{BufRead,BufReader,Read,Write},os::unix::{fs::{DirBuilderExt,MetadataExt,OpenOptionsExt,PermissionsExt},net::{UnixListener,UnixStream}},os::fd::AsRawFd,path::{Path,PathBuf},process::{Command,Stdio},time::{Duration,Instant}};
const UNAVAILABLE:&str="活动同步尚未接入当前官方窗口；任务结束后请正常退出 Codex，再从 ClawKit 打开";
fn endpoint(home:&Path)->PathBuf{home.join("clawkit-runtime/activity.sock")}
fn private(path:&Path)->bool{std::fs::symlink_metadata(path).is_ok_and(|m|m.uid()==unsafe{libc::geteuid()}&&m.mode()&0o077==0&&!m.file_type().is_symlink())}
pub fn request(home:&Path,action:&str,thread:Option<&str>)->Result<Value,String>{
    let path=endpoint(home);
    if !private(path.parent().ok_or(UNAVAILABLE)?)||!private(&path){return Err(UNAVAILABLE.into())}
    let mut stream=UnixStream::connect(path).map_err(|_|UNAVAILABLE)?;
    stream.set_read_timeout(Some(Duration::from_secs(20))).map_err(|e|e.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(2))).map_err(|e|e.to_string())?;
    writeln!(stream,"{}",json!({"action":action,"threadId":thread})).map_err(|_|UNAVAILABLE)?;
    let mut data=String::new();BufReader::new(stream).take(4*1024*1024).read_line(&mut data).map_err(|_|UNAVAILABLE)?;
    let result:Value=serde_json::from_str(&data).map_err(|_|UNAVAILABLE)?;
    if let Some(error)=result["error"].as_str(){return Err(error.to_owned())}
    result.get("result").cloned().ok_or(UNAVAILABLE.into())
}
pub fn ready(home:&Path)->bool{request(home,"status",None).is_ok_and(|v|v["ready"]==true)}
pub fn maybe_run(){
    if std::env::args().nth(1).as_deref()!=Some("--clawkit-window-host"){return}
    let args:Vec<_>=std::env::args_os().collect();
    let result=if args.len()==5{serve(Path::new(&args[2]),Path::new(&args[3]),Path::new(&args[4]))}else{Err("启动参数无效".into())};
    std::process::exit(if result.is_ok(){0}else{2});
}
pub fn launch(home:&Path,adapter:&Path,app:&Path)->Result<(),String>{
    let _child=Command::new(adapter).arg("--clawkit-window-host").arg(home).arg(adapter).arg(app)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|_|"无法启动窗口同步")?;
    // Readiness is reported by the service. Never stop official work on timeout.
    let deadline=Instant::now()+Duration::from_secs(30);
    while Instant::now()<deadline {
        if ready(home){return Ok(())}
        std::thread::sleep(Duration::from_millis(200));
    }
    Err("Codex 已启动，但活动同步尚未就绪，请稍后重试连接".into())
}
fn serve(home:&Path,adapter:&Path,app:&Path)->Result<(),String>{
    let dir=home.join("clawkit-runtime");
    if !dir.exists(){std::fs::DirBuilder::new().mode(0o700).create(&dir).map_err(|e|e.to_string())?;}
    if !private(&dir){return Err("窗口通道目录权限不符".into())}
    let lock=std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(dir.join("activity.lock")).map_err(|e|e.to_string())?;
    if unsafe{libc::flock(lock.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)}!=0{return Err("窗口同步已运行".into())}
    let path=endpoint(home);
    if path.exists(){if !private(&path)||UnixStream::connect(&path).is_ok(){return Err("已有窗口通道".into())}std::fs::remove_file(&path).map_err(|e|e.to_string())?;}
    let listener=UnixListener::bind(&path).map_err(|e|e.to_string())?;
    std::fs::set_permissions(&path,std::fs::Permissions::from_mode(0o600)).map_err(|e|e.to_string())?;
    listener.set_nonblocking(true).map_err(|e|e.to_string())?;
    let executable=app.join("Contents/MacOS").join(app.file_stem().ok_or("应用路径无效")?);
    let engine=app.join("Contents/Resources/codex-cli/bin/codex");
    let mut cmd=Command::new(executable);
    cmd.arg("--remote-debugging-pipe").env("CODEX_HOME",home).env("CODEX_CLI_PATH",adapter).env("CLAWKIT_CODEX_ENGINE",engine)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(profile)=std::env::var_os("CODEX_ELECTRON_USER_DATA_PATH"){
        if !Path::new(&profile).is_absolute(){return Err("配置目录必须为绝对路径".into())}
        cmd.arg(format!("--user-data-dir={}",Path::new(&profile).display()));
    }
    let(mut pipe,mut child)=pipe::Pipe::launch(&mut cmd)?;
    let mut session:Option<String>=None;
    let mut init_error=None;
    let mut retry=Instant::now();
    loop {
        if child.try_wait().map_err(|e|e.to_string())?.is_some(){break}
        if session.is_none()&&Instant::now()>=retry {
            let mut initialize=||->Result<String,String>{
                let targets=pipe.call("Target.getTargets",json!({}),None)?;
                let id=targets["targetInfos"].as_array().and_then(|v|v.iter().find(|v|v["type"]=="page"&&v["url"]=="app://-/index.html"))
                    .and_then(|v|v["targetId"].as_str()).ok_or("等待官方窗口")?;
                let attached=pipe.call("Target.attachToTarget",json!({"targetId":id,"flatten":true}),None)?;
                let id=attached["sessionId"].as_str().ok_or("官方窗口连接失败")?.to_owned();
                bind::install(&mut pipe,&id)?;Ok(id)
            };
            match initialize(){Ok(id)=>{session=Some(id);init_error=None},Err(error)=>init_error=Some(error)}
            retry=Instant::now()+Duration::from_secs(3);
        }
        match listener.accept(){
            Ok((mut stream,_))=>{
                let _=stream.set_read_timeout(Some(Duration::from_secs(2)));
                let _=stream.set_write_timeout(Some(Duration::from_secs(2)));
                let mut line=String::new();
                let result=(||->Result<Value,String>{
                    BufReader::new(&stream).take(4096).read_line(&mut line).map_err(|_|"请求不完整")?;
                    let req:Value=serde_json::from_str(&line).map_err(|_|"请求无效")?;
                    let action=req["action"].as_str().ok_or("操作缺失")?;
                    if action=="status"{return Ok(json!({"ready":session.is_some(),"reason":init_error}))}
                    if !matches!(action,"read"|"open"|"clear"|"close"|"markRead"){return Err("不支持的窗口操作".into())}
                    if action=="markRead"&&req["threadId"].as_str().is_none_or(|id|uuid::Uuid::parse_str(id).is_err()){return Err("任务 ID 无效".into())}
                    let id=session.as_deref().ok_or(UNAVAILABLE)?;
                    let expression=format!("globalThis.__clawkitActivity({},{})",json!(action),req["threadId"]);
                    match pipe.evaluate(id,&expression) {
                        Ok(value)=>Ok(value),
                        Err(error)=>{session=None;retry=Instant::now();Err(error)}
                    }
                })();
                let response=match result{Ok(result)=>json!({"result":result}),Err(error)=>json!({"error":error})};
                let _=writeln!(stream,"{response}");
            },
            Err(e) if e.kind()==std::io::ErrorKind::WouldBlock=>std::thread::sleep(Duration::from_millis(25)),
            Err(_)=>break,
        }
    }
    let _=std::fs::remove_file(path);Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Requires a disposable official desktop profile; never points at the live user profile"]
    fn official_activity_read_ack_and_retained_order() {
        let home=PathBuf::from(std::env::var_os("CLAWKIT_ACTIVITY_TEST_HOME").expect("isolated home"));
        assert!(home.to_string_lossy().starts_with("/tmp/ck-rea-"));
        assert_ne!(home,std::env::var_os("HOME").map(PathBuf::from).unwrap().join(".codex"));
        let profile=std::env::var_os("CLAWKIT_ACTIVITY_TEST_PROFILE").expect("isolated profile");
        let mut cmd=Command::new("/Applications/ChatGPT.app/Contents/MacOS/ChatGPT");
        cmd.arg("--remote-debugging-pipe").arg(format!("--user-data-dir={}",Path::new(&profile).display()))
            .env("CODEX_HOME",&home).env("CODEX_ELECTRON_USER_DATA_PATH",profile)
            .env_remove("CODEX_CLI_PATH").env_remove("CLAWKIT_CODEX_ENGINE")
            .stdout(Stdio::null()).stderr(Stdio::null());
        let(mut pipe,child)=pipe::Pipe::launch(&mut cmd).unwrap();
        struct Cleanup(std::process::Child);
        impl Drop for Cleanup {fn drop(&mut self){unsafe{libc::kill(self.0.id() as i32,libc::SIGTERM);}let _=self.0.wait();}}
        let _child=Cleanup(child);
        let deadline=Instant::now()+Duration::from_secs(30);
        let session=loop{
            assert!(Instant::now()<deadline,"Official window did not load");
            let targets=pipe.call("Target.getTargets",json!({}),None).unwrap();
            if let Some(target)=targets["targetInfos"].as_array().unwrap().iter().find(|t|t["url"]=="app://-/index.html"){
                let attached=pipe.call("Target.attachToTarget",json!({"targetId":target["targetId"],"flatten":true}),None).unwrap();
                let session=attached["sessionId"].as_str().unwrap().to_owned();
                if bind::install(&mut pipe,&session).is_ok(){break session}
            }
            std::thread::sleep(Duration::from_millis(300));
        };
        let id=uuid::Uuid::new_v4().to_string();
        let mark=format!(r#"(async()=>{{const m=await import('app://-/assets/app-shared-8bd9d0399038.js');const a=await m.fN.threadReadState.open(()=>{{}});if(a.status!=='ready')return {{ready:false}};try {{const r=await a.session.set({{hostId:'local',threadId:{},hasUnreadTurn:true}});return {{ready:true,written:r.status==='ok',unread:m.dQt.isUnread('local',{})}}}}finally{{await a.session.unsubscribe()}}}})()"#,json!(id),json!(id));
        let before=pipe.evaluate(&session,&mark).unwrap();
        assert_eq!(before,json!({"ready":true,"written":true,"unread":true}));
        let cleared=pipe.evaluate(&session,&format!("__clawkitActivity('markRead',{})",json!(id))).unwrap();
        assert_eq!(cleared["read"],true);
        let opened=pipe.evaluate(&session,"__clawkitActivity('open')").unwrap();
        let again=pipe.evaluate(&session,"__clawkitActivity('open')").unwrap();
        assert_eq!(opened["activatedAt"],again["activatedAt"]);
        assert_eq!(opened["priority"],again["priority"]);
        let clear=pipe.evaluate(&session,"__clawkitActivity('clear')").unwrap();
        assert_eq!(opened["activatedAt"],clear["activatedAt"]);
        assert_eq!(pipe.evaluate(&session,"__clawkitActivity('close')").unwrap()["open"],false);
        assert_eq!(pipe.evaluate(&session,"__clawkitActivity('open')").unwrap()["open"],true);
        println!("Official renderer: true unread -> confirmed read; retained session -> clear -> close/reopen passed");
    }
}
