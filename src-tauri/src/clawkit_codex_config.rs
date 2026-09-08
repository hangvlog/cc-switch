use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use toml_edit::{DocumentMut, Item, Table};

const PROVIDER_ID: &str = "clawkit";
const MODEL_CATALOG_FILE: &str = "clawkit-models.json";
const BACKUP_MANIFEST_FILE: &str = "manifest.json";
const MAX_BACKUPS: usize = 10;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClawkitCodexConfigurationStatus {
    pub configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    pub config_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_quota: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_quota: Option<i64>,
    pub can_rollback: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupManifest {
    config_existed: bool,
    catalog_existed: bool,
}

#[derive(Clone)]
struct FileSnapshot {
    path: PathBuf,
    contents: Option<Vec<u8>>,
}

impl FileSnapshot {
    fn capture(path: PathBuf) -> Result<Self, String> {
        let contents = match std::fs::read(&path) {
            Ok(contents) => Some(contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("读取 {} 失败：{error}", path.display())),
        };
        Ok(Self { path, contents })
    }

    fn restore(&self) -> Result<(), String> {
        match &self.contents {
            Some(contents) => crate::config::atomic_write_private(&self.path, contents)
                .map_err(|error| error.to_string()),
            None => match std::fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!("恢复 {} 失败：{error}", self.path.display())),
            },
        }
    }
}

pub fn status() -> ClawkitCodexConfigurationStatus {
    status_from_home(&crate::codex_config::get_codex_config_dir())
}

pub fn apply(
    gateway: &crate::clawkit_gateway::GatewayBootstrap,
) -> Result<ClawkitCodexConfigurationStatus, String> {
    let home = crate::codex_config::get_codex_config_dir();
    apply_at_home(gateway, &home)
}

fn apply_at_home(
    gateway: &crate::clawkit_gateway::GatewayBootstrap,
    home: &Path,
) -> Result<ClawkitCodexConfigurationStatus, String> {
    let config_path = home.join("config.toml");
    let catalog_path = home.join(MODEL_CATALOG_FILE);
    let config_snapshot = FileSnapshot::capture(config_path.clone())?;
    let catalog_snapshot = FileSnapshot::capture(catalog_path.clone())?;
    let existing = config_snapshot
        .contents
        .as_deref()
        .map(String::from_utf8_lossy)
        .map(|value| value.into_owned())
        .unwrap_or_default();
    let default_model = crate::clawkit_gateway::preferred_default_model(&gateway.models)
        .ok_or_else(|| "当前账号没有可用的 API 模型".to_string())?;
    let updated = managed_config_text(
        &existing,
        &gateway.base_url,
        &gateway.api_key,
        default_model,
    )?;
    let backup_dir = create_backup(home, &config_snapshot, &catalog_snapshot)?;

    let write_result = (|| {
        crate::clawkit_gateway::write_model_catalog_at(&gateway.models, catalog_path.clone())?;
        write_config_at(&config_path, &updated)
    })();
    if let Err(error) = write_result {
        let config_restore = config_snapshot.restore();
        let catalog_restore = catalog_snapshot.restore();
        let backup_cleanup = std::fs::remove_dir_all(&backup_dir);
        if let Err(restore_error) = config_restore.and(catalog_restore) {
            return Err(format!("{error}；同时回滚失败：{restore_error}"));
        }
        if let Err(cleanup_error) = backup_cleanup {
            log::warn!(
                "Unable to remove unused ClawKit Codex backup {}: {cleanup_error}",
                backup_dir.display()
            );
        }
        return Err(error);
    }

    Ok(ClawkitCodexConfigurationStatus {
        configured: true,
        model: Some(default_model.to_string()),
        models: gateway.models.clone(),
        config_path: config_path.to_string_lossy().to_string(),
        available_quota: Some(gateway.available_quota),
        used_quota: Some(gateway.used_quota),
        can_rollback: latest_backup_dir(home).is_some(),
    })
}

fn write_config_at(path: &Path, contents: &str) -> Result<(), String> {
    if !contents.trim().is_empty() {
        contents
            .parse::<DocumentMut>()
            .map_err(|error| format!("Codex config.toml 格式无效：{error}"))?;
    }
    crate::config::atomic_write_private(path, contents.as_bytes())
        .map_err(|error| error.to_string())
}

pub fn rollback() -> Result<ClawkitCodexConfigurationStatus, String> {
    let home = crate::codex_config::get_codex_config_dir();
    rollback_at_home(&home)
}

fn rollback_at_home(home: &Path) -> Result<ClawkitCodexConfigurationStatus, String> {
    let backup_dir =
        latest_backup_dir(home).ok_or_else(|| "没有可恢复的 Codex 配置备份".to_string())?;
    let manifest: BackupManifest = serde_json::from_slice(
        &std::fs::read(backup_dir.join(BACKUP_MANIFEST_FILE))
            .map_err(|error| format!("读取 Codex 配置备份失败：{error}"))?,
    )
    .map_err(|error| format!("Codex 配置备份清单无效：{error}"))?;
    restore_backup_file(
        &backup_dir.join("config.toml"),
        &home.join("config.toml"),
        manifest.config_existed,
    )?;
    restore_backup_file(
        &backup_dir.join(MODEL_CATALOG_FILE),
        &home.join(MODEL_CATALOG_FILE),
        manifest.catalog_existed,
    )?;
    std::fs::remove_dir_all(&backup_dir)
        .map_err(|error| format!("清理已恢复的 Codex 配置备份失败：{error}"))?;
    Ok(status_from_home(home))
}

fn create_backup(
    home: &Path,
    config: &FileSnapshot,
    catalog: &FileSnapshot,
) -> Result<PathBuf, String> {
    let root = backup_root(home);
    std::fs::create_dir_all(&root)
        .map_err(|error| format!("创建 Codex 配置备份目录失败：{error}"))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis();
    let backup_dir = root.join(format!("backup-{timestamp}"));
    std::fs::create_dir_all(&backup_dir)
        .map_err(|error| format!("创建 Codex 配置备份失败：{error}"))?;
    if let Some(contents) = &config.contents {
        crate::config::atomic_write_private(&backup_dir.join("config.toml"), contents)
            .map_err(|error| error.to_string())?;
    }
    if let Some(contents) = &catalog.contents {
        crate::config::atomic_write_private(&backup_dir.join(MODEL_CATALOG_FILE), contents)
            .map_err(|error| error.to_string())?;
    }
    let manifest = serde_json::to_vec_pretty(&BackupManifest {
        config_existed: config.contents.is_some(),
        catalog_existed: catalog.contents.is_some(),
    })
    .map_err(|error| error.to_string())?;
    crate::config::atomic_write_private(&backup_dir.join(BACKUP_MANIFEST_FILE), &manifest)
        .map_err(|error| error.to_string())?;
    cleanup_old_backups(&root)?;
    Ok(backup_dir)
}

fn backup_root(home: &Path) -> PathBuf {
    home.join("clawkit-backups")
}

fn latest_backup_dir(home: &Path) -> Option<PathBuf> {
    let mut entries = std::fs::read_dir(backup_root(home))
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join(BACKUP_MANIFEST_FILE).is_file())
        .collect::<Vec<_>>();
    entries.sort();
    entries.pop()
}

fn cleanup_old_backups(root: &Path) -> Result<(), String> {
    let mut entries = std::fs::read_dir(root)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    entries.sort();
    let remove_count = entries.len().saturating_sub(MAX_BACKUPS);
    for path in entries.into_iter().take(remove_count) {
        std::fs::remove_dir_all(&path)
            .map_err(|error| format!("清理旧 Codex 配置备份失败：{error}"))?;
    }
    Ok(())
}

fn restore_backup_file(source: &Path, target: &Path, existed: bool) -> Result<(), String> {
    if existed {
        let contents = std::fs::read(source)
            .map_err(|error| format!("读取 {} 失败：{error}", source.display()))?;
        crate::config::atomic_write_private(target, &contents).map_err(|error| error.to_string())
    } else {
        match std::fs::remove_file(target) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("恢复 {} 失败：{error}", target.display())),
        }
    }
}

fn status_from_home(home: &Path) -> ClawkitCodexConfigurationStatus {
    let config_path = home.join("config.toml");
    let catalog_path = home.join(MODEL_CATALOG_FILE);
    let contents = std::fs::read_to_string(&config_path).unwrap_or_default();
    let parsed = contents.parse::<DocumentMut>().ok();
    let model = parsed
        .as_ref()
        .and_then(|doc| doc.get("model"))
        .and_then(Item::as_str)
        .map(str::to_string);
    let configured = parsed.as_ref().is_some_and(|doc| {
        doc.get("model_provider").and_then(Item::as_str) == Some(PROVIDER_ID)
            && provider_has_value(doc, "base_url")
            && provider_has_value(doc, "experimental_bearer_token")
            && doc.get("model_catalog_json").and_then(Item::as_str) == Some(MODEL_CATALOG_FILE)
            && catalog_path.is_file()
    });
    let models = std::fs::read(&catalog_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| {
            value
                .get("models")
                .and_then(|models| models.as_array())
                .cloned()
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|model| {
            model
                .get("slug")
                .and_then(|slug| slug.as_str())
                .map(str::to_string)
        })
        .collect();

    ClawkitCodexConfigurationStatus {
        configured,
        model,
        models,
        config_path: config_path.to_string_lossy().to_string(),
        available_quota: None,
        used_quota: None,
        can_rollback: latest_backup_dir(home).is_some(),
    }
}

fn provider_has_value(doc: &DocumentMut, key: &str) -> bool {
    doc.get("model_providers")
        .and_then(Item::as_table)
        .and_then(|providers| providers.get(PROVIDER_ID))
        .and_then(Item::as_table)
        .and_then(|provider| provider.get(key))
        .and_then(Item::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

fn managed_config_text(
    existing: &str,
    base_url: &str,
    api_key: &str,
    default_model: &str,
) -> Result<String, String> {
    let mut doc = if existing.trim().is_empty() {
        DocumentMut::new()
    } else {
        existing
            .parse::<DocumentMut>()
            .map_err(|error| format!("Codex config.toml 格式无效：{error}"))?
    };
    doc["model_provider"] = toml_edit::value(PROVIDER_ID);
    doc["model"] = toml_edit::value(default_model);
    doc["model_catalog_json"] = toml_edit::value(MODEL_CATALOG_FILE);
    // ClawKit currently bridges Responses requests to an upstream Chat
    // Completions endpoint that rejects Codex's hosted web_search tool. The
    // model catalog flag is descriptive only; this top-level setting is what
    // prevents Codex from adding web_search to requests at runtime.
    doc[crate::codex_config::CODEX_WEB_SEARCH_FIELD] =
        toml_edit::value(crate::codex_config::CODEX_WEB_SEARCH_DISABLED);
    if doc
        .get("model_providers")
        .and_then(Item::as_table)
        .is_none()
    {
        doc["model_providers"] = Item::Table(Table::new());
    }
    let providers = doc["model_providers"]
        .as_table_mut()
        .ok_or_else(|| "Codex config.toml 的 model_providers 不是表".to_string())?;
    if providers
        .get(PROVIDER_ID)
        .and_then(Item::as_table)
        .is_none()
    {
        providers[PROVIDER_ID] = Item::Table(Table::new());
    }
    let provider = providers[PROVIDER_ID]
        .as_table_mut()
        .ok_or_else(|| "Codex config.toml 的 ClawKit 供应商不是表".to_string())?;
    provider["name"] = toml_edit::value("ClawKit API");
    provider["base_url"] = toml_edit::value(base_url.trim_end_matches('/'));
    provider["wire_api"] = toml_edit::value("responses");
    provider["requires_openai_auth"] = toml_edit::value(false);
    provider["experimental_bearer_token"] = toml_edit::value(api_key);
    Ok(doc.to_string())
}

#[cfg(test)]
#[path = "clawkit_codex_config_tests.rs"]
mod tests;
