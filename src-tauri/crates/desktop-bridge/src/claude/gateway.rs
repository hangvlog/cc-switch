//! How a phone turn reaches the same model route as the desktop app: the
//! applied third-party gateway profile (written by CC Switch / ClawKit) and
//! the CLI build the desktop pinned. Nothing is inherited from this process.
use super::index::Paths;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
    time::{Duration, Instant},
};

pub struct Gateway {
    pub base_url: String,
    pub token: String,
    pub models: Vec<String>,
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

pub fn gateway(paths: &Paths) -> Result<Gateway, String> {
    let library = paths.app_data.as_ref().ok_or("未找到 Claude 桌面端数据")?.join("configLibrary");
    let meta = read_json(&library.join("_meta.json")).ok_or("Claude 桌面端未配置第三方模型网关")?;
    let applied = meta["appliedId"].as_str().filter(|id| super::index::is_uuid(id)).ok_or("Claude 桌面端未启用模型网关配置")?;
    let profile = read_json(&library.join(format!("{applied}.json"))).ok_or("Claude 桌面端网关配置不可读")?;
    if profile["inferenceProvider"] != "gateway" {
        return Err("Claude 桌面端当前不是网关模式，手机续聊暂不支持".into());
    }
    let base_url = profile["inferenceGatewayBaseUrl"].as_str().filter(|u| u.starts_with("http")).ok_or("网关地址缺失")?;
    let token = profile["inferenceGatewayApiKey"].as_str().filter(|t| !t.is_empty()).ok_or("网关凭据缺失")?;
    let models = profile["inferenceModels"].as_array().into_iter().flatten()
        .filter_map(|m| m["name"].as_str().map(Into::into)).collect();
    Ok(Gateway { base_url: base_url.into(), token: token.into(), models })
}

fn version_key(name: &str) -> Vec<u64> {
    name.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// The CLI build the desktop runs (`claude-code/<version>/<hash>/...`).
pub fn cli(paths: &Paths, pinned: Option<&str>) -> Result<PathBuf, String> {
    if let Some(path) = paths.cli_override.as_ref().filter(|p| p.is_file()) {
        return Ok(path.clone());
    }
    let root = paths.app_data.as_ref().ok_or("未找到 Claude 桌面端数据")?.join("claude-code");
    let mut versions: Vec<String> = fs::read_dir(&root).map_err(|_| "未找到 Claude Code 运行程序")?
        .flatten().filter_map(|e| e.file_name().to_str().map(Into::into)).collect();
    versions.sort_by_key(|v| version_key(v));
    if let Some(pin) = pinned.filter(|p| versions.iter().any(|v| v == p)) {
        versions.retain(|v| v == pin);
    }
    let binary = if cfg!(windows) { "claude.exe" } else { "claude" };
    for version in versions.iter().rev() {
        for build in fs::read_dir(root.join(version)).into_iter().flatten().flatten() {
            for candidate in [build.path().join("claude.app/Contents/MacOS").join(binary), build.path().join(binary)] {
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }
    Err("未找到 Claude Code 运行程序".into())
}

/// launchd starts services with a minimal PATH; tools the session uses
/// (git, node, brew) live on the user's login-shell PATH.
pub fn shell_path() -> String {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        let fallback = format!("{}:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            std::env::var("PATH").unwrap_or_default());
        if cfg!(windows) {
            return std::env::var("PATH").unwrap_or_default();
        }
        let shell = std::env::var("SHELL").ok().filter(|s| s.starts_with('/')).unwrap_or_else(|| "/bin/zsh".into());
        let Ok(mut child) = Command::new(shell).args(["-ilc", "printf %s \"$PATH\""])
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() else { return fallback };
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = child.try_wait() {
                let mut out = String::new();
                use std::io::Read;
                let _ = child.stdout.take().map(|mut s| s.read_to_string(&mut out));
                let line = out.lines().last().unwrap_or("").trim().to_string();
                return if line.contains("/usr/bin") { line } else { fallback };
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        fallback
    }).clone()
}

/// Environment for the CLI child. PROVIDER_MANAGED_BY_HOST stops the `env`
/// block of ~/.claude/settings.json from re-pointing the provider.
pub fn child_env(gateway: &Gateway) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = ["HOME", "USER", "LOGNAME", "LANG", "TMPDIR", "SHELL", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "SystemRoot"]
        .iter().filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v))).collect();
    env.push(("PATH".into(), shell_path()));
    env.push(("ANTHROPIC_BASE_URL".into(), gateway.base_url.clone()));
    env.push(("ANTHROPIC_AUTH_TOKEN".into(), gateway.token.clone()));
    env.push(("CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST".into(), "1".into()));
    env
}
