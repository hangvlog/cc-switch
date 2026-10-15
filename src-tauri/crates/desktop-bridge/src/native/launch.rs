use std::{path::Path};
/// Configure only this launch. Never mutate Codex, global environment, credentials or config.
pub fn launch(home: &Path, adapter: &Path) -> Result<(), String> {
    if !home.is_absolute() || !adapter.is_absolute() || !adapter.is_file() {
        return Err("Codex 启动路径无效".into());
    }
    if crate::renderer::request(home,"status",None).is_ok_and(|v|v["running"]==true) {
        return Ok(());
    }
    if crate::version::running_pid(home).is_some() {
        return Err("Codex 正在运行。请先保存草稿并正常退出 Codex，再点击“打开 Codex”；不会中断正在执行的任务。".into());
    }
    if std::env::var_os("CODEX_CLI_PATH").is_some_and(|path| {
        Path::new(&path).canonicalize().ok() != adapter.canonicalize().ok()
    }) {
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
    crate::router_launch::ensure_before_launch(home, adapter)?;
    crate::renderer::launch(home, adapter, app)
}
