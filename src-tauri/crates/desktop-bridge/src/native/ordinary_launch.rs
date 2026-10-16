//! Opt-in bridge for applications started through the user's normal macOS entry points.
//! launchd environment affects future launches only; never restarts an existing Codex.
use std::{path::{Path, PathBuf}, process::Command};

const KEYS: [&str; 2] = ["CODEX_CLI_PATH", "CLAWKIT_CODEX_ENGINE"];

fn get(key: &str) -> Result<Option<String>, String> {
    let output = Command::new("/bin/launchctl").args(["getenv", key]).output()
        .map_err(|_| "无法读取普通启动配置")?;
    // launchctl exits 1 for an unset variable.
    if output.status.code() == Some(1) && output.stdout.is_empty() { return Ok(None); }
    if !output.status.success() { return Err("无法读取普通启动配置".into()); }
    let value = String::from_utf8(output.stdout).map_err(|_| "启动路径编码无效")?;
    let value = value.trim_end_matches('\n');
    Ok((!value.is_empty()).then(|| value.to_owned()))
}

fn set(key: &str, value: Option<&str>) -> Result<(), String> {
    let mut command = Command::new("/bin/launchctl");
    match value { Some(value) => { command.args(["setenv", key, value]); },
        None => { command.args(["unsetenv", key]); } }
    if command.status().map_err(|_| "无法更新普通启动配置")?.success() { Ok(()) }
    else { Err("系统未能保存普通启动配置".into()) }
}

pub(crate) fn engine() -> Result<PathBuf, String> {
    ["/Applications/ChatGPT.app", "/Applications/Codex.app"].iter()
        .map(|p| Path::new(p).join("Contents/Resources/codex-cli/bin/codex"))
        .find(|p| p.is_file()).ok_or_else(|| "请先安装 Codex 桌面端".into())
}

fn owned(values: &[Option<String>; 2], adapter: &str) -> bool {
    values[0].as_deref() == Some(adapter) && values[1].as_deref().is_some_and(|p|
        ["/Applications/ChatGPT.app", "/Applications/Codex.app"].iter().any(|app|
            p == format!("{app}/Contents/Resources/codex-cli/bin/codex")))
}

/// Respect an existing engine override. The opt-in belongs only to this installed adapter.
pub fn configure(adapter: &Path, enabled: bool) -> Result<(), String> {
    if !adapter.is_absolute() || !adapter.is_file() { return Err("普通启动程序路径无效".into()); }
    let adapter = adapter.to_str().ok_or("普通启动程序路径编码无效")?;
    let previous = [get(KEYS[0])?, get(KEYS[1])?];
    if !enabled {
        // Never erase an override changed by the user or another application.
        if owned(&previous, adapter) {
            set(KEYS[0], None)?;
            set(KEYS[1], None)?;
        }
        return Ok(());
    }
    if previous.iter().any(Option::is_some) && !owned(&previous, adapter) {
        return Err("已有自定义 Codex 启动配置，未覆盖；原桌面接续仍可使用".into());
    }
    let engine = engine()?;
    let engine = engine.to_str().ok_or("Codex 路径编码无效")?;
    if previous[0].as_deref() == Some(adapter) && previous[1].as_deref() == Some(engine) { return Ok(()); }
    // Engine first: there is never an adapter-only intermediate launch configuration.
    set(KEYS[1], Some(engine))?;
    if let Err(error) = set(KEYS[0], Some(adapter)) {
        let _ = set(KEYS[1], previous[1].as_deref());
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordinary_launch_owns_only_its_exact_override() {
        let adapter = "/Applications/ClawKit Desktop.app/Contents/MacOS/cc-switch";
        let engine = "/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex";
        assert!(owned(&[Some(adapter.into()), Some(engine.into())], adapter));
        assert!(!owned(&[Some("/custom/codex".into()), Some(engine.into())], adapter));
        assert!(!owned(&[Some(adapter.into()), Some("/custom/engine".into())], adapter));
        assert!(!owned(&[None, None], adapter));
    }
}
