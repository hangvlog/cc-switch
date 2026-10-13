//! Local-only named pipe, protected by the current user's SID and checked on connect.
use std::{ffi::c_void,fs::File,io::{self,Read,Write},os::windows::{io::{AsRawHandle,FromRawHandle},fs::OpenOptionsExt},path::{Path,PathBuf},process::Command,time::{Duration,Instant}};
use sha2::{Digest,Sha256};
type Handle=*mut c_void;
#[repr(C)] struct SecurityAttributes {length:u32, descriptor:*mut c_void, inherit:i32}
#[link(name="kernel32")] extern "system" {
    fn CreateNamedPipeW(name:*const u16,open:u32,mode:u32,max:u32,out:u32,input:u32,timeout:u32,security:*const SecurityAttributes)->Handle;
    fn ConnectNamedPipe(pipe:Handle,overlapped:*mut c_void)->i32;
    fn SetNamedPipeHandleState(pipe:Handle,mode:*const u32,collection:*const u32,timeout:*const u32)->i32;
    fn GetNamedPipeServerProcessId(pipe:Handle,pid:*mut u32)->i32;
    fn GetCurrentProcess()->Handle;
    fn OpenProcess(access:u32,inherit:i32,pid:u32)->Handle;
    fn CloseHandle(handle:Handle)->i32;
    fn LocalFree(memory:*mut c_void)->*mut c_void;
}
#[link(name="advapi32")] extern "system" {
    fn OpenProcessToken(process:Handle,access:u32,token:*mut Handle)->i32;
    fn GetTokenInformation(token:Handle,class:u32,data:*mut c_void,len:u32,needed:*mut u32)->i32;
    fn ConvertSidToStringSidW(sid:*mut c_void,text:*mut *mut u16)->i32;
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(text:*const u16,revision:u32,descriptor:*mut *mut c_void,size:*mut u32)->i32;
}
fn wide(text:&str)->Vec<u16>{text.encode_utf16().chain(Some(0)).collect()}
fn sid(process:Handle)->io::Result<String>{unsafe{
    let mut token=std::ptr::null_mut();
    if OpenProcessToken(process,8,&mut token)==0{return Err(io::Error::last_os_error());}
    let mut needed=0;GetTokenInformation(token,1,std::ptr::null_mut(),0,&mut needed);
    // Align TOKEN_USER storage to pointers.
    let mut data=vec![0usize;(needed as usize+7)/8];
    let ok=GetTokenInformation(token,1,data.as_mut_ptr().cast(),needed,&mut needed);CloseHandle(token);
    if ok==0{return Err(io::Error::last_os_error());}
    let mut ptr=std::ptr::null_mut();
    if ConvertSidToStringSidW(data[0] as *mut c_void,&mut ptr)==0{return Err(io::Error::last_os_error());}
    let mut len=0;while *ptr.add(len)!=0{len+=1;}
    let value=String::from_utf16_lossy(std::slice::from_raw_parts(ptr,len));LocalFree(ptr.cast());Ok(value)
}}
pub fn socket_path(home:&Path)->PathBuf {
    let owner=unsafe{sid(GetCurrentProcess())}.unwrap_or_default();
    let path=home.canonicalize().unwrap_or_else(|_|home.into());
    PathBuf::from(format!(r"\\.\pipe\clawkit-engine-{:x}",Sha256::digest(format!("{}:{owner}",path.to_string_lossy().to_lowercase()).as_bytes())))
}
pub struct Stream { file:File, read_timeout:Duration, write_timeout:Duration }
impl Stream {
    fn new(file:File)->Self {Self{file,read_timeout:Duration::from_secs(5),write_timeout:Duration::from_secs(5)}}
    pub fn connect(path:&Path)->io::Result<Self>{
        let deadline=Instant::now()+Duration::from_secs(2);
        let file=loop{match std::fs::OpenOptions::new().read(true).write(true).security_qos_flags(0x10000).open(path){
            Ok(file)=>break file,
            Err(e) if e.raw_os_error()==Some(231) && Instant::now()<deadline=>std::thread::sleep(Duration::from_millis(10)),
            Err(e)=>return Err(e),
        }};
        unsafe {
            let mut pid=0;
            if GetNamedPipeServerProcessId(file.as_raw_handle(),&mut pid)==0{return Err(io::Error::last_os_error());}
            let process=OpenProcess(0x1000,0,pid);
            if process.is_null(){return Err(io::Error::last_os_error());}
            let server=sid(process);CloseHandle(process);
            if server?!=sid(GetCurrentProcess())? {return Err(io::Error::new(io::ErrorKind::PermissionDenied,"Pipe owner mismatch"));}
            let mode=1;
            if SetNamedPipeHandleState(file.as_raw_handle(),&mode,std::ptr::null(),std::ptr::null())==0{return Err(io::Error::last_os_error());}
        }
        Ok(Self::new(file))
    }
    pub fn try_clone(&self)->io::Result<Self>{Ok(Self{file:self.file.try_clone()?,read_timeout:self.read_timeout,write_timeout:self.write_timeout})}
    pub fn set_read_timeout(&mut self,value:Option<Duration>)->io::Result<()>{self.read_timeout=value.unwrap_or(Duration::from_secs(35));Ok(())}
    pub fn set_write_timeout(&mut self,value:Option<Duration>)->io::Result<()>{self.write_timeout=value.unwrap_or(Duration::from_secs(5));Ok(())}
}
impl Read for Stream {fn read(&mut self,buf:&mut[u8])->io::Result<usize>{
    let deadline=Instant::now()+self.read_timeout;
    if buf.is_empty(){return Ok(0);}
    loop{match self.file.read(buf){
        Ok(0) if Instant::now()<deadline=>std::thread::sleep(Duration::from_millis(5)),
        Ok(0)=>return Err(io::Error::new(io::ErrorKind::TimedOut,"Pipe read timed out")),
        Err(e) if e.raw_os_error()==Some(232) && Instant::now()<deadline=>std::thread::sleep(Duration::from_millis(5)),
        Err(e) if e.raw_os_error()==Some(232)=>return Err(io::Error::new(io::ErrorKind::TimedOut,"Pipe read timed out")),
        value=>return value,
    }}
}}
impl Write for Stream {fn write(&mut self,buf:&[u8])->io::Result<usize>{
    let deadline=Instant::now()+self.write_timeout;
    loop{match self.file.write(buf){Ok(0) if !buf.is_empty() && Instant::now()<deadline=>std::thread::sleep(Duration::from_millis(5)),value=>return value}}
}fn flush(&mut self)->io::Result<()>{Ok(())}}
pub struct Listener{path:PathBuf,first:Option<File>}
fn create(path:&Path,first:bool)->io::Result<File>{unsafe{
    let owner=sid(GetCurrentProcess())?;let sddl=wide(&format!("D:P(A;;GA;;;{owner})"));
    let mut descriptor=std::ptr::null_mut();
    if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(),1,&mut descriptor,std::ptr::null_mut())==0{return Err(io::Error::last_os_error());}
    let security=SecurityAttributes{length:std::mem::size_of::<SecurityAttributes>() as u32,descriptor,inherit:0};
    let handle=CreateNamedPipeW(wide(&path.to_string_lossy()).as_ptr(),3|if first{0x80000}else{0},1|8,16,65536,65536,5000,&security);
    let error=io::Error::last_os_error();LocalFree(descriptor);
    if handle as isize == -1{return Err(error);}
    Ok(File::from_raw_handle(handle))
}}
impl Listener {pub fn incoming(mut self)->impl Iterator<Item=io::Result<Stream>>{
    std::iter::from_fn(move||{
        let file=match self.first.take().map(Ok).unwrap_or_else(||create(&self.path,false)){Ok(v)=>v,Err(e)=>return Some(Err(e))};
        loop {let result=unsafe{ConnectNamedPipe(file.as_raw_handle(),std::ptr::null_mut())};
            if result!=0{return Some(Ok(Stream::new(file)));}
            match io::Error::last_os_error().raw_os_error(){Some(535)=>return Some(Ok(Stream::new(file))),Some(536)=>std::thread::sleep(Duration::from_millis(10)),_=>return Some(Err(io::Error::last_os_error()))}
        }
    })
}}
pub fn bind(home:&Path)->Result<(Listener,File,PathBuf),String>{let path=socket_path(home);let first=create(&path,true).map_err(|e|e.to_string())?;let guard=first.try_clone().map_err(|e|e.to_string())?;Ok((Listener{path:path.clone(),first:Some(first)},guard,path))}
pub fn fallback(command:&mut Command)->i32{command.status().ok().and_then(|s|s.code()).unwrap_or(2)}
pub fn cleanup(_path:&Path){}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn current_user_pipe_roundtrips_repeated_connections_and_locks_owner() {
        let root=tempfile::tempdir().unwrap();
        let (listener,guard,path)=bind(root.path()).unwrap();
        assert!(bind(root.path()).is_err());
        let server=std::thread::spawn(move||{
            let _guard=guard;
            for stream in listener.incoming().take(3) {
                let mut stream=stream.unwrap();
                let line=super::super::transport::read_line(&mut std::io::BufReader::new(stream.try_clone().unwrap()),100).unwrap().unwrap();
                assert_eq!(line,b"hello\n");
                stream.write_all(b"world\n").unwrap();
            }
        });
        for _ in 0..3 {
            let mut client=Stream::connect(&path).unwrap();
            client.write_all(b"hello\n").unwrap();
            let line=super::super::transport::read_line(&mut std::io::BufReader::new(client),100).unwrap().unwrap();
            assert_eq!(line,b"world\n");
        }
        server.join().unwrap();
    }
}
