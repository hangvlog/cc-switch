//! Chromium's inherited debugging pipe: no TCP listener or remotely supplied JS.
use serde_json::{json, Value};
use std::{fs::File, io::{Read, Write}, os::fd::{AsRawFd, FromRawFd}, process::{Child, Command}, time::{Duration, Instant}};

pub(super) struct Pipe { input:File, output:File, pending:Vec<u8>, seq:u64 }
fn pair()->Result<(File,File),String>{
    let mut fds=[0;2];
    if unsafe{libc::pipe(fds.as_mut_ptr())}!=0{return Err("无法建立窗口管道".into())}
    // Keep all originals above stdio/CDP descriptors before pre_exec dup2.
    let mut copies=[0;2];
    for i in 0..2 { copies[i]=unsafe{libc::fcntl(fds[i],libc::F_DUPFD_CLOEXEC,10)}; }
    for fd in fds {unsafe{libc::close(fd);}}
    if copies.iter().any(|fd|*fd<0){for fd in copies {if fd>=0 {unsafe{libc::close(fd);}}}return Err("无法复制窗口管道".into())}
    Ok(unsafe{(File::from_raw_fd(copies[0]),File::from_raw_fd(copies[1]))})
}
impl Pipe {
    pub fn launch(command:&mut Command)->Result<(Self,Child),String>{
        use std::os::unix::process::CommandExt;
        let (child_input,input)=pair()?; let(output,child_output)=pair()?;
        let (r,w)=(child_input.as_raw_fd(),child_output.as_raw_fd());
        unsafe {command.pre_exec(move||{
            if libc::dup2(r,3)<0 || libc::dup2(w,4)<0{return Err(std::io::Error::last_os_error())}
            Ok(())
        });}
        let child=command.spawn().map_err(|_|"无法启动官方应用")?;
        Ok((Self{input,output,pending:Vec::new(),seq:0},child))
    }
    pub fn call(&mut self,method:&str,params:Value,session:Option<&str>)->Result<Value,String>{
        self.seq+=1;let id=self.seq;
        let mut msg=json!({"id":id,"method":method,"params":params});
        if let Some(session)=session{msg["sessionId"]=json!(session);}
        let mut bytes=serde_json::to_vec(&msg).map_err(|e|e.to_string())?;bytes.push(0);
        self.input.write_all(&bytes).map_err(|_|"官方窗口管道已断开")?;
        let deadline=Instant::now()+Duration::from_secs(15);
        loop {
            if let Some(end)=self.pending.iter().position(|c|*c==0){
                let packet:Vec<_>=self.pending.drain(..=end).collect();
                let value:Value=serde_json::from_slice(&packet[..end]).map_err(|_|"窗口响应格式无效")?;
                if value["id"]==id {
                    if value.get("error").is_some(){return Err("官方窗口协议调用失败".into())}
                    return Ok(value["result"].clone())
                }
                continue
            }
            if self.pending.len()>32*1024*1024{return Err("窗口响应超过范围".into())}
            let remaining=deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero(){return Err("官方窗口响应超时".into())}
            let mut fd=libc::pollfd{fd:self.output.as_raw_fd(),events:libc::POLLIN,revents:0};
            if unsafe{libc::poll(&mut fd,1,remaining.as_millis().min(i32::MAX as u128) as i32)}<=0{return Err("官方窗口响应超时".into())}
            let mut buf=[0;65536];let count=self.output.read(&mut buf).map_err(|_|"窗口读取失败")?;
            if count==0{return Err("官方窗口已退出".into())}self.pending.extend_from_slice(&buf[..count]);
        }
    }
    pub fn evaluate(&mut self,session:&str,expression:&str)->Result<Value,String>{
        let result=self.call("Runtime.evaluate",json!({"expression":expression,"awaitPromise":true,"returnByValue":true}),Some(session))?;
        if result.get("exceptionDetails").is_some(){return Err("官方窗口状态读取失败".into())}
        Ok(result["result"]["value"].clone())
    }
}
