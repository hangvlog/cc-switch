//! Installation identity survives authentication expiry and explicit logout.
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::Path;

pub(super) fn load_or_create(session_path: &Path) -> Result<String, String> {
    let path = session_path.with_extension("device-id");
    match fs::read_to_string(&path) {
        Ok(value) => return validate(value),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(format!("无法读取 ClawKit 设备编号: {error}")),
    }
    // Migrate even an expired session before any caller deletes its credentials.
    let legacy = fs::read_to_string(session_path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|session| session["device_id"].as_str().map(str::to_owned))
        .filter(|value| !value.trim().is_empty());
    let device_id = legacy.unwrap_or_else(|| format!("clawkit-desktop-{}", uuid::Uuid::new_v4()));
    let parent = path.parent().ok_or("ClawKit 设备编号路径无效")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
    }
    temporary
        .write_all(device_id.as_bytes())
        .map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    // Publish atomically without overwriting an identity another process just created.
    match temporary.persist_noclobber(&path) {
        Ok(_) => Ok(device_id),
        Err(error) if error.error.kind() == ErrorKind::AlreadyExists => {
            validate(fs::read_to_string(&path).map_err(|error| error.to_string())?)
        }
        Err(error) => Err(format!("无法保存 ClawKit 设备编号: {}", error.error)),
    }
}

fn validate(value: String) -> Result<String, String> {
    if value.trim().is_empty() {
        Err("ClawKit 设备编号无效，请恢复设备编号文件".to_string())
    } else {
        Ok(value)
    }
}
