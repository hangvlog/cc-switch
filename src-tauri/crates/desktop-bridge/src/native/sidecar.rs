//! Unix launch handoff: the official engine keeps the desktop as its parent.
//! A separate relay inherits only stdio and two anonymous pipe descriptors.
use std::{
    fs::File,
    os::fd::{AsRawFd, FromRawFd},
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
};

fn pair() -> Result<(File, File), String> {
    let mut descriptors = [0; 2];
    if unsafe { libc::pipe(descriptors.as_mut_ptr()) } != 0 {
        return Err("无法建立原引擎管道".into());
    }
    let copies = descriptors.map(|fd| unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 10) });
    for fd in descriptors {
        unsafe {
            libc::close(fd);
        }
    }
    if copies.iter().any(|fd| *fd < 0) {
        for fd in copies {
            if fd >= 0 {
                unsafe {
                    libc::close(fd);
                }
            }
        }
        return Err("无法复制原引擎管道".into());
    }
    Ok(unsafe { (File::from_raw_fd(copies[0]), File::from_raw_fd(copies[1])) })
}

pub(super) fn launch(mut engine: Command, home: &Path) -> Result<i32, String> {
    let (engine_input, relay_input) = pair()?;
    let (relay_output, engine_output) = pair()?;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut relay = Command::new(executable);
    relay
        .arg("--clawkit-engine-relay")
        .arg(home)
        .env_remove("CODEX_CLI_PATH")
        .env_remove("CLAWKIT_CODEX_ENGINE")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let (input, output) = (relay_input.as_raw_fd(), relay_output.as_raw_fd());
    unsafe {
        relay.pre_exec(move || {
            if libc::dup2(input, 3) < 0 || libc::dup2(output, 4) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = relay.spawn().map_err(|e| e.to_string())?;
    drop(relay_input);
    drop(relay_output);
    // CommandExt::exec replaces this process, retaining its PID and parent.
    // The engine executable and its signatures remain the official installed bytes.
    engine
        .stdin(Stdio::from(engine_input))
        .stdout(Stdio::from(engine_output));
    let error = engine.exec();
    let _ = child.kill();
    let _ = child.wait();
    Err(format!("无法启动官方 Codex：{error}"))
}

pub(super) fn inherited() -> Result<(File, File), String> {
    for descriptor in [3, 4] {
        if unsafe { libc::fcntl(descriptor, libc::F_GETFD) } < 0 {
            return Err("远控收发缺少继承管道".into());
        }
        // Child utilities must not inherit a live engine input/output channel.
        if unsafe { libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err("无法保护继承管道".into());
        }
    }
    Ok(unsafe { (File::from_raw_fd(3), File::from_raw_fd(4)) })
}
