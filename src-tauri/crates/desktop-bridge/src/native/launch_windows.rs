//! Discover the signed installed desktop; set overrides only on this child process.
use std::{path::Path,process::Command,os::windows::process::CommandExt};
pub fn launch(home:&Path,adapter:&Path)->Result<(),String> {
    if !home.is_absolute() || !adapter.is_absolute() || !adapter.is_file(){return Err("Codex 启动路径无效".into());}
    if super::ready(home){return Ok(());}
    let query=Command::new("powershell.exe").args(["-NoProfile","-NonInteractive","-Command",
        "if(Get-Process ChatGPT,Codex -ErrorAction SilentlyContinue){Write-Output 'RUNNING';exit}; $p=Get-AppxPackage OpenAI.Codex | Select-Object -First 1; if($p){Join-Path $p.InstallLocation 'app'}"])
        .creation_flags(0x08000000).output().map_err(|_|"无法检查官方桌面安装")?;
    if !query.status.success(){return Err("无法查询官方桌面安装位置".into());}
    let value=String::from_utf8_lossy(&query.stdout).trim().trim_start_matches('\u{feff}').to_string();
    if value=="RUNNING"{return Err("Codex 正在运行。请保存草稿并正常退出后再打开；不会中断现有任务。".into());}
    let app=Path::new(&value);
    let desktop=app.join("ChatGPT.exe");let engine=app.join("resources/codex.exe");
    if !desktop.is_file() || !engine.is_file(){return Err("未找到兼容的官方 Codex 桌面安装".into());}
    let mut command=Command::new(desktop);
    command.env("CODEX_HOME",home).env("CODEX_CLI_PATH",adapter).env("CLAWKIT_CODEX_ENGINE",engine);
    if let Some(profile)=std::env::var_os("CODEX_ELECTRON_USER_DATA_PATH") {
        if !Path::new(&profile).is_absolute(){return Err("Codex 数据目录必须为绝对路径".into());}
        command.arg(format!("--user-data-dir={}",Path::new(&profile).display()));
    }
    command.spawn().map_err(|_|"无法启动官方桌面")?;Ok(())
}
