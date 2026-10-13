//! Recovery entry point runs before the desktop WebView and instance lock.
use std::path::PathBuf;
pub fn maybe_run() {
    let action=std::env::args().nth(1).unwrap_or_default();
    if !matches!(action.as_str(),"--clawkit-open-codex"|"--clawkit-codex-status") {return}
    let result=(||->Result<serde_json::Value,String>{
        if std::env::args().len()!=2 {return Err("启动入口不接受额外参数".into())}
        let home=std::env::var_os("CODEX_HOME").map(PathBuf::from)
            .or_else(||std::env::var_os("HOME").map(|p|PathBuf::from(p).join(".codex"))).ok_or("找不到用户目录")?;
        if action=="--clawkit-open-codex" {
            let adapter=std::env::current_exe().map_err(|_|"找不到启动程序")?;
            crate::native::launch(&home,&adapter)?;
        }
        Ok(crate::version::status(&home))
    })();
    match result {
        Ok(status)=>{println!("{}",status);std::process::exit(0)},
        Err(error)=>{eprintln!("{}",serde_json::json!({"error":error}));std::process::exit(1)}
    }
}
