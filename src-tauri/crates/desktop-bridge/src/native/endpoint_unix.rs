//! User-only Unix socket and single-engine lock.
use super::transport::check_private;
use std::{path::{Path,PathBuf}, os::unix::{fs::{OpenOptionsExt,PermissionsExt,DirBuilderExt},io::AsRawFd,process::CommandExt},process::Command};
pub type Stream=std::os::unix::net::UnixStream;
pub fn bind(home:&Path)->Result<(std::os::unix::net::UnixListener,std::fs::File,PathBuf),String> {
    let socket=super::socket_path(home); let directory=socket.parent().ok_or("通道路径无效")?;
    if !directory.exists() {std::fs::DirBuilder::new().mode(0o700).create(directory).map_err(|e|e.to_string())?;}
    check_private(directory,true)?;
    let lock=std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).mode(0o600).custom_flags(libc::O_NOFOLLOW)
        .open(directory.join("engine.lock")).map_err(|e|e.to_string())?;
    if unsafe{libc::flock(lock.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)}!=0 {return Err("另一个引擎已持有通道".into());}
    if socket.exists(){check_private(&socket,false)?;std::fs::remove_file(&socket).map_err(|e|e.to_string())?;}
    let listener=std::os::unix::net::UnixListener::bind(&socket).map_err(|e|e.to_string())?;
    std::fs::set_permissions(&socket,std::fs::Permissions::from_mode(0o600)).map_err(|e|e.to_string())?;
    Ok((listener,lock,socket))
}
pub fn fallback(command:&mut Command)->i32 {let _=command.exec();2}
pub fn cleanup(path:&Path){let _=std::fs::remove_file(path);}
