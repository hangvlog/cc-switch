//! One-time, copy-only migration from CC Switch into ClawKit-owned storage.
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::error::AppError;
use rusqlite::{
    backup::{Backup, StepResult},
    Connection, OpenFlags,
};
use serde_json::{json, Value};

const MARKER: &str = "clawkit-storage-v1.json";

pub(crate) fn initialize(app: &tauri::AppHandle) -> Result<(), AppError> {
    crate::app_store::refresh_app_config_dir_override(app);
    let dest = crate::config::get_app_config_dir();
    validate_destination(&dest)?;
    if dest.join(MARKER).exists() || dest.join("clawkit.db").exists() {
        return Ok(());
    }
    let home = crate::config::get_home_dir();
    let source = match crate::app_store::legacy_app_config_dir(app)? {
        Some(path) => path,
        None => legacy_default(&home),
    };
    migrate(
        &source,
        &home.join(".cc-switch/settings.json"),
        &crate::config::get_app_config_dir(),
    )?;
    Ok(())
}

fn legacy_default(home: &Path) -> PathBuf {
    let default = home.join(".cc-switch");
    #[cfg(windows)]
    if std::env::var_os("CC_SWITCH_TEST_HOME").is_none() && !default.join("cc-switch.db").exists() {
        if let Some(old_home) = std::env::var_os("HOME") {
            let old = PathBuf::from(old_home).join(".cc-switch");
            if old.join("cc-switch.db").exists() {
                return old;
            }
        }
    }
    default
}

/// Prevent selecting a CC Switch directory, including aliases and descendants.
pub(crate) fn validate_destination(path: &Path) -> Result<(), AppError> {
    if !path.is_absolute() {
        return Err(AppError::Config("ClawKit 数据目录必须使用绝对路径".into()));
    }
    let canonical = if path.exists() {
        fs::canonicalize(path).map_err(|e| AppError::io(path, e))?
    } else if let Some(parent) = path.parent().filter(|p| p.exists()) {
        fs::canonicalize(parent)
            .map_err(|e| AppError::io(parent, e))?
            .join(
                path.file_name()
                    .ok_or_else(|| AppError::Config("无效数据目录".into()))?,
            )
    } else {
        path.to_path_buf()
    };
    if canonical.ancestors().any(|p| {
        p.file_name()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(".cc-switch"))
            || p.join("cc-switch.db").exists()
    }) {
        return Err(AppError::Config(
            "请选择独立的 ClawKit 数据目录，不能使用 CC Switch 目录或其子目录".into(),
        ));
    }
    Ok(())
}

pub(crate) fn migrate(
    source: &Path,
    legacy_settings: &Path,
    dest: &Path,
) -> Result<bool, AppError> {
    validate_destination(dest)?;
    if dest.join(MARKER).exists() || dest.join("clawkit.db").exists() {
        return Ok(false); // Never merge or overwrite an established ClawKit profile.
    }
    if dest.exists()
        && fs::read_dir(dest)
            .map_err(|e| AppError::io(dest, e))?
            .next()
            .is_some()
    {
        return Err(AppError::Config(format!(
            "ClawKit 目标目录非空且未完成迁移，请先保全并检查目录：{}",
            dest.display()
        )));
    }
    let parent = dest
        .parent()
        .ok_or_else(|| AppError::Config("数据目录缺少父目录".into()))?;
    fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    let stage = tempfile::Builder::new()
        .prefix(".clawkit-migration-")
        .tempdir_in(parent)
        .map_err(|e| AppError::io(parent, e))?;
    private_permissions(stage.path(), true)?;
    if source.exists() {
        let root = fs::canonicalize(source).map_err(|e| AppError::io(source, e))?;
        let stage_path =
            fs::canonicalize(stage.path()).map_err(|e| AppError::io(stage.path(), e))?;
        if stage_path.starts_with(&root) {
            return Err(AppError::Config(
                "ClawKit 目标目录不能位于旧数据目录内".into(),
            ));
        }
        copy_tree(&root, stage.path(), &root, &mut Vec::new())?;
        let old_db = stage.path().join("cc-switch.db");
        if old_db.exists() {
            fs::rename(&old_db, stage.path().join("clawkit.db"))
                .map_err(|e| AppError::io(&old_db, e))?;
        }
    }
    // Old releases stored device settings in the default home even with a DB override.
    if legacy_settings.exists() {
        copy_file(legacy_settings, &stage.path().join("settings.json"))?;
    }
    isolate_sync_settings(&stage.path().join("settings.json"))?;
    let record = json!({"version":1,"source":source,"database":"clawkit.db",
        "completedAt":chrono::Utc::now().to_rfc3339(),"sourcePreserved":true,
        "cloudSyncRequiresReview":true});
    let marker = stage.path().join(MARKER);
    fs::write(&marker, serde_json::to_vec_pretty(&record).unwrap())
        .map_err(|e| AppError::io(&marker, e))?;
    private_permissions(&marker, false)?;
    // Publish only after every file and the consistent SQLite snapshot succeeded.
    // An interrupted copy leaves the destination absent; a later launch retries.
    if dest.exists() {
        fs::remove_dir(dest).map_err(|e| AppError::io(dest, e))?; // Only an empty destination.
    }
    fs::rename(stage.path(), dest).map_err(|e| AppError::io(dest, e))?;
    Ok(true)
}

fn copy_tree(
    source: &Path,
    dest: &Path,
    root: &Path,
    ancestors: &mut Vec<PathBuf>,
) -> Result<(), AppError> {
    let canonical = fs::canonicalize(source).map_err(|e| AppError::io(source, e))?;
    if !canonical.starts_with(root) || ancestors.contains(&canonical) {
        return Err(AppError::Config(format!(
            "迁移遇到外部链接或循环链接，请先检查：{}",
            source.display()
        )));
    }
    if source.is_dir() {
        fs::create_dir_all(dest).map_err(|e| AppError::io(dest, e))?;
        private_permissions(dest, true)?;
        ancestors.push(canonical);
        for entry in fs::read_dir(source).map_err(|e| AppError::io(source, e))? {
            let entry = entry.map_err(|e| AppError::io(source, e))?;
            let name = entry.file_name();
            let text = name.to_string_lossy();
            if ["-wal", "-shm", "-journal"].iter().any(|suffix| {
                text.strip_suffix(suffix)
                    .is_some_and(|base| source.join(base).is_file())
            }) {
                continue; // Captured through the SQLite backup API, never raw-copied.
            }
            copy_tree(&entry.path(), &dest.join(name), root, ancestors)?;
        }
        ancestors.pop();
        Ok(())
    } else if source.is_file() {
        copy_file(source, dest)
    } else {
        Err(AppError::Config(format!(
            "无法迁移特殊文件：{}",
            source.display()
        )))
    }
}

fn copy_file(source: &Path, dest: &Path) -> Result<(), AppError> {
    if source
        .extension()
        .is_some_and(|ext| ext == "db" || ext == "sqlite" || ext == "sqlite3")
    {
        let conn = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| AppError::Database(format!("读取迁移源失败 {}: {e}", source.display())))?;
        let mut target = Connection::open(dest).map_err(|e| AppError::Database(e.to_string()))?;
        {
            let backup =
                Backup::new(&conn, &mut target).map_err(|e| AppError::Database(e.to_string()))?;
            match backup
                .step(-1)
                .map_err(|e| AppError::Database(e.to_string()))?
            {
                StepResult::Done => {}
                _ => {
                    return Err(AppError::Database(
                        "数据库正在写入，请关闭 CC Switch 后重试迁移".into(),
                    ))
                }
            }
        }
        let check: String = target
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .map_err(|e| AppError::Database(e.to_string()))?;
        if check != "ok" {
            return Err(AppError::Database("迁移快照完整性校验失败".into()));
        }
        // Make the published image self-contained even if the source used WAL.
        target
            .pragma_update(None, "journal_mode", "DELETE")
            .map_err(|e| AppError::Database(e.to_string()))?;
        target
            .close()
            .map_err(|(_, e)| AppError::Database(e.to_string()))?;
    } else {
        fs::copy(source, dest).map_err(|e| AppError::io(source, e))?;
    }
    private_permissions(dest, false)
}

fn isolate_sync_settings(path: &Path) -> Result<(), AppError> {
    if !path.exists() {
        return Ok(());
    }
    let bytes = fs::read(path).map_err(|e| AppError::io(path, e))?;
    let mut settings: Value =
        serde_json::from_slice(&bytes).map_err(|e| AppError::json(path, e))?;
    for key in ["webdavSync", "s3Sync", "webdavBackup"] {
        if let Some(sync) = settings.get_mut(key).and_then(Value::as_object_mut) {
            sync.insert("enabled".into(), false.into());
            sync.insert("autoSync".into(), false.into());
            sync.insert("status".into(), json!({}));
        }
    }
    fs::write(path, serde_json::to_vec_pretty(&settings).unwrap())
        .map_err(|e| AppError::io(path, e))
}

fn private_permissions(path: &Path, directory: bool) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Preserve executable bits on scripts/skills, restrict access to the owner.
        let mode = if directory {
            0o700
        } else {
            0o600
                | (fs::metadata(path)
                    .map_err(|e| AppError::io(path, e))?
                    .permissions()
                    .mode()
                    & 0o100)
        };
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|e| AppError::io(path, e))?;
    }
    #[cfg(not(unix))]
    let _ = (path, directory); // Inherit the destination user's Windows ACL.
    Ok(())
}
