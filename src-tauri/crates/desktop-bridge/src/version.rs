use serde_json::{json, Value};
use std::path::Path;

// Private IPC is not a stable vendor API. Unknown builds remain read-only.
const VERIFIED_BUILD: &str = "26.924.22138:11645";

#[cfg(target_os = "macos")]
fn running_build(home: &Path) -> Option<String> {
    use std::process::Command;
    let output = Command::new("/usr/sbin/lsof")
        .args(["-a", "-U", "-Fp"])
        .arg(home.join("ipc/ipc.sock"))
        .output()
        .ok()?;
    let pids = String::from_utf8(output.stdout).ok()?;
    let pid = pids.lines().find_map(|line| line.strip_prefix('p'))?;
    if !pid.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let output = Command::new("/bin/ps")
        .args(["-p", pid, "-o", "comm="])
        .output()
        .ok()?;
    let executable = String::from_utf8(output.stdout).ok()?;
    let contents = Path::new(executable.trim()).parent()?.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    let read = |key: &str| -> Option<String> {
        let value = Command::new("/usr/bin/plutil")
            .args(["-extract", key, "raw", "-o", "-"])
            .arg(contents.join("Info.plist"))
            .output()
            .ok()?;
        if !value.status.success() {
            return None;
        }
        Some(String::from_utf8(value.stdout).ok()?.trim().into())
    };
    Some(format!(
        "{}:{}",
        read("CFBundleShortVersionString")?,
        read("CFBundleVersion")?
    ))
}

#[cfg(not(target_os = "macos"))]
fn running_build(_home: &Path) -> Option<String> {
    None
}

pub fn can_send(home: &Path) -> bool {
    running_build(home).as_deref() == Some(VERIFIED_BUILD)
}

pub fn status(home: &Path) -> Value {
    let build = running_build(home);
    json!({"mode":"desktop-owner","build":build,"verifiedBuild":VERIFIED_BUILD,
        "canSend":build.as_deref()==Some(VERIFIED_BUILD),"codexHome":home,
        "socketAvailable":home.join("ipc/ipc.sock").exists()})
}
