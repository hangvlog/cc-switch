use std::{path::Path, process::Command};
/// Configure only this launch. Never mutate Codex, global environment, credentials or config.
pub fn launch(home: &Path, adapter: &Path) -> Result<(), String> {
    if !home.is_absolute() || !adapter.is_absolute() || !adapter.is_file() {
        return Err("Codex 启动路径无效".into());
    }
    if super::ready(home) {
        return Ok(());
    }
    if crate::version::running_pid(home).is_some() {
        return Err("Codex 正在运行。请先保存草稿并正常退出 Codex，再点击“打开 Codex”；不会中断正在执行的任务。".into());
    }
    if std::env::var_os("CODEX_CLI_PATH").is_some() {
        return Err("已设置自定义 Codex 引擎，请先移除该启动覆盖；不会自动替换".into());
    }
    let app = ["/Applications/ChatGPT.app", "/Applications/Codex.app"]
        .iter()
        .map(Path::new)
        .find(|p| p.is_dir())
        .ok_or("请先安装 Codex 桌面端")?;
    let engine = app.join("Contents/Resources/codex-cli/bin/codex");
    if !engine.is_file() {
        return Err("此 Codex 安装结构尚未适配，未更改配置".into());
    }
    let mut command = Command::new("/usr/bin/open");
    command.arg("-n");
    // LaunchServices does not inherit the caller's environment. Preserve an
    // explicitly isolated desktop profile without touching global settings.
    if let Some(profile) = std::env::var_os("CODEX_ELECTRON_USER_DATA_PATH") {
        let path = Path::new(&profile);
        if !path.is_absolute() {
            return Err("Codex 数据目录必须为绝对路径".into());
        }
        command.arg("--env").arg(format!(
            "CODEX_ELECTRON_USER_DATA_PATH={}",
            path.display()
        ));
    }
    let status = command
        .arg("--env")
        .arg(format!("CODEX_HOME={}", home.display()))
        .arg("--env")
        .arg(format!("CODEX_CLI_PATH={}", adapter.display()))
        .arg("--env")
        .arg(format!("CLAWKIT_CODEX_ENGINE={}", engine.display()))
        .arg(app)
        .status()
        .map_err(|_| "无法启动 Codex")?;
    if status.success() {
        Ok(())
    } else {
        Err("Codex 启动失败，原配置未更改".into())
    }
}
