use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) fn codex_command(binary: &Path) -> Command {
    #[cfg(target_os = "windows")]
    if binary
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
        })
    {
        let mut command = Command::new("cmd.exe");
        command.arg("/D").arg("/S").arg("/C").arg(binary);
        return command;
    }
    Command::new(binary)
}

pub(super) fn find_codex_cli(codex_home: Option<&Path>) -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("CODEX_REMOTE_CODEX_BIN") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Some(path);
        }
    }

    configured_codex_cli_candidate(codex_home)
        .into_iter()
        .chain(bundled_codex_cli_candidates())
        .chain(platform_codex_cli_candidates())
        .chain(codex_cli_candidates_on_path(std::env::var_os("PATH")))
        .find(|candidate| candidate.is_file())
        .or_else(|| {
            // A GUI launch from Finder or Explorer only inherits a minimal
            // PATH, so fall back to the shared list of well-known install
            // locations (Homebrew, nvm/fnm/volta/asdf/mise shims, npm
            // prefixes, %APPDATA%\npm, ...).
            crate::codex_config::codex_cli_candidates()
                .into_iter()
                .find(|candidate| candidate.is_absolute() && candidate.is_file())
        })
}

fn configured_codex_cli_candidate(codex_home: Option<&Path>) -> Option<PathBuf> {
    let config_path = codex_home
        .map(|directory| directory.join("config.toml"))
        .unwrap_or_else(crate::codex_config::get_codex_config_path);
    let config = std::fs::read_to_string(config_path).ok()?;
    let document = config.parse::<toml::Value>().ok()?;
    let raw_path = document
        .get("mcp_servers")?
        .get("node_repl")?
        .get("env")?
        .get("CODEX_CLI_PATH")?
        .as_str()?
        .trim();
    if raw_path.is_empty() {
        return None;
    }

    let candidate = PathBuf::from(raw_path);
    let expected_name = candidate.file_name()?;
    if !candidate.is_absolute()
        || !codex_binary_names()
            .iter()
            .any(|name| expected_name == std::ffi::OsStr::new(name))
    {
        return None;
    }
    candidate.is_file().then_some(candidate)
}

fn codex_binary_names() -> &'static [&'static str] {
    if cfg!(target_os = "windows") {
        &["codex.exe", "codex.cmd", "codex.bat"]
    } else {
        &["codex"]
    }
}

fn bundled_codex_cli_candidates() -> Vec<PathBuf> {
    let Ok(current) = std::env::current_exe() else {
        return Vec::new();
    };
    let Some(directory) = current.parent() else {
        return Vec::new();
    };
    codex_binary_names()
        .iter()
        .map(|name| directory.join(name))
        .collect()
}

fn codex_cli_candidates_on_path(path: Option<std::ffi::OsString>) -> Vec<PathBuf> {
    let Some(path) = path else {
        return Vec::new();
    };
    std::env::split_paths(&path)
        .flat_map(|directory| {
            codex_binary_names()
                .iter()
                .map(move |name| directory.join(name))
        })
        .collect()
}

fn append_versioned_codex_candidates(
    candidates: &mut Vec<PathBuf>,
    versions_root: &Path,
    binary_name: &str,
) {
    let Ok(entries) = std::fs::read_dir(versions_root) else {
        return;
    };
    let mut versioned = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.path().join(binary_name))
        .filter(|candidate| candidate.is_file())
        .collect::<Vec<_>>();
    versioned.sort_by(|left, right| right.cmp(left));
    candidates.extend(versioned);
}

fn platform_codex_cli_candidates() -> Vec<PathBuf> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let mut candidates = Vec::new();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let candidates = Vec::new();
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = dirs::home_dir() {
            for app in [
                "ChatGPT.app",
                "Codex.app",
                "OpenAI Codex.app",
                "OpenAI.Codex.app",
            ] {
                candidates.push(
                    home.join("Applications")
                        .join(app)
                        .join("Contents/Resources/codex"),
                );
            }
        }
        for app in [
            "ChatGPT.app",
            "Codex.app",
            "OpenAI Codex.app",
            "OpenAI.Codex.app",
        ] {
            candidates.push(
                PathBuf::from("/Applications")
                    .join(app)
                    .join("Contents/Resources/codex"),
            );
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let local = PathBuf::from(local);
            let codex_root = local.join("OpenAI/Codex");
            let codex_bin = codex_root.join("bin");
            candidates.push(codex_bin.join("codex.exe"));
            append_versioned_codex_candidates(&mut candidates, &codex_bin, "codex.exe");
            for root in [
                codex_root,
                local.join("Programs/OpenAI/Codex"),
                local.join("Programs/ChatGPT"),
            ] {
                candidates.push(root.join("resources/codex.exe"));
                candidates.push(root.join("Resources/codex.exe"));
            }
        }
        for variable in ["ProgramFiles", "ProgramW6432"] {
            if let Some(program_files) = std::env::var_os(variable) {
                append_windows_store_codex_candidates(
                    &mut candidates,
                    &PathBuf::from(program_files).join("WindowsApps"),
                );
            }
        }
    }
    candidates
}

#[cfg(target_os = "windows")]
fn append_windows_store_codex_candidates(candidates: &mut Vec<PathBuf>, root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name.starts_with("openai.codex_")
            || name.starts_with("openai.codexbeta_")
            || name.starts_with("openai.chatgpt_")
            || name.starts_with("openai.chatgpt-desktop_")
        {
            candidates.push(entry.path().join("app/resources/codex.exe"));
            candidates.push(entry.path().join("app/Resources/codex.exe"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        append_versioned_codex_candidates, bundled_codex_cli_candidates,
        codex_cli_candidates_on_path, configured_codex_cli_candidate,
    };
    use std::fs;

    #[test]
    fn bundled_candidates_stay_next_to_the_executable() {
        let current = std::env::current_exe().expect("current exe");
        let directory = current.parent().expect("exe parent");
        let candidates = bundled_codex_cli_candidates();
        assert!(!candidates.is_empty());
        assert!(candidates
            .iter()
            .all(|candidate| candidate.parent() == Some(directory)));
    }

    #[test]
    fn finds_global_codex_cli_candidates_without_executing_them() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let separator = if cfg!(target_os = "windows") {
            ";"
        } else {
            ":"
        };
        let second = tempfile::tempdir().expect("create second temp dir");
        let path = std::ffi::OsString::from(format!(
            "{}{}{}",
            temp.path().display(),
            separator,
            second.path().display()
        ));
        let candidates = codex_cli_candidates_on_path(Some(path));
        assert!(candidates.iter().any(|item| item.starts_with(temp.path())));
        assert!(candidates
            .iter()
            .any(|item| item.starts_with(second.path())));
    }

    #[test]
    fn reads_codex_desktop_cli_path_from_config() {
        let codex_home = tempfile::tempdir().expect("create Codex home");
        let binary = codex_home.path().join(super::codex_binary_names()[0]);
        fs::write(&binary, b"stub").expect("create Codex CLI stub");
        let encoded = serde_json::to_string(binary.to_str().expect("UTF-8 path"))
            .expect("encode config path");
        fs::write(
            codex_home.path().join("config.toml"),
            format!("[mcp_servers.node_repl.env]\nCODEX_CLI_PATH = {encoded}\n"),
        )
        .expect("write config");

        assert_eq!(
            configured_codex_cli_candidate(Some(codex_home.path())),
            Some(binary)
        );
    }

    #[test]
    fn finds_codex_cli_in_versioned_desktop_bin_directory() {
        let root = tempfile::tempdir().expect("create Codex bin root");
        let binary = root.path().join("8e5b6932251c2c1c").join("codex.exe");
        fs::create_dir_all(binary.parent().expect("binary parent"))
            .expect("create version directory");
        fs::write(&binary, b"stub").expect("create Codex CLI stub");

        let mut candidates = Vec::new();
        append_versioned_codex_candidates(&mut candidates, root.path(), "codex.exe");

        assert_eq!(candidates, vec![binary]);
    }
}
