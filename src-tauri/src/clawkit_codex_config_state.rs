use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;

use crate::app_config::AppType;
use crate::provider::{Provider, ProviderMeta};
use crate::store::AppState;

use super::{
    apply_at_home_with_state, latest_backup_dir, read_backup_manifest, remove_backup_dir,
    restore_backup_files, status_from_home, ClawkitCodexConfigurationStatus, PROVIDER_ID,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BackupCcSwitchState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_clawkit_provider: Option<Provider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_local_current: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_db_current: Option<String>,
    previous_takeover_enabled: bool,
}

pub async fn apply(
    gateway: &crate::clawkit_gateway::GatewayBootstrap,
    state: &AppState,
) -> Result<ClawkitCodexConfigurationStatus, String> {
    let app_type = AppType::Codex;
    // One-click setup and CC Switch both own the same Live config.toml. Keep
    // the provider switch lock until Live + provider state agree.
    let switch_guard = state
        .proxy_service
        .lock_switch_for_app(app_type.as_str())
        .await;
    let home = crate::codex_config::get_codex_config_dir();
    let previous_state = capture_cc_switch_state(state, &app_type).await?;

    if previous_state.previous_takeover_enabled {
        if let Err(error) = state.proxy_service.disable_takeover_for_app_sync(&app_type) {
            drop(switch_guard);
            let restore_error = restore_takeover_if_needed(state, &previous_state)
                .await
                .err();
            return Err(combine_rollback_error(error, restore_error));
        }
    }

    let (configured, backup_dir) =
        match apply_at_home_with_state(gateway, &home, Some(previous_state.clone())) {
            Ok(result) => result,
            Err(error) => {
                let mut rollback_errors =
                    restore_cc_switch_state(state, &app_type, &previous_state);
                drop(switch_guard);
                if let Err(restore_error) = restore_takeover_if_needed(state, &previous_state).await
                {
                    rollback_errors.push(restore_error);
                }
                return Err(combine_rollback_errors(error, rollback_errors));
            }
        };
    let config_text = match std::fs::read_to_string(home.join("config.toml")) {
        Ok(config) => config,
        Err(error) => {
            let error = format!("读取已生成的 Codex 配置失败：{error}");
            return rollback_failed_apply(
                error,
                &home,
                &backup_dir,
                state,
                &app_type,
                &previous_state,
                switch_guard,
            )
            .await;
        }
    };
    let mut provider = Provider::with_id(
        PROVIDER_ID.to_string(),
        "ClawKit API".to_string(),
        json!({
            "auth": {},
            "config": config_text,
        }),
        Some("https://clawkit.chat".to_string()),
    );
    provider.category = Some("custom".to_string());
    provider.meta = Some(ProviderMeta {
        api_format: Some("openai_responses".to_string()),
        ..Default::default()
    });

    let state_result = (|| {
        state
            .db
            .save_provider(app_type.as_str(), &provider)
            .map_err(|error| error.to_string())?;
        crate::settings::set_current_provider(&app_type, Some(PROVIDER_ID))
            .map_err(|error| error.to_string())?;
        state
            .db
            .set_current_provider(app_type.as_str(), PROVIDER_ID)
            .map_err(|error| error.to_string())?;
        Ok::<(), String>(())
    })();

    if let Err(error) = state_result {
        return rollback_failed_apply(
            error,
            &home,
            &backup_dir,
            state,
            &app_type,
            &previous_state,
            switch_guard,
        )
        .await;
    }

    Ok(configured)
}

pub async fn rollback(state: &AppState) -> Result<ClawkitCodexConfigurationStatus, String> {
    let app_type = AppType::Codex;
    let switch_guard = state
        .proxy_service
        .lock_switch_for_app(app_type.as_str())
        .await;
    let home = crate::codex_config::get_codex_config_dir();
    let backup_dir =
        latest_backup_dir(&home).ok_or_else(|| "没有可恢复的 Codex 配置备份".to_string())?;
    let manifest = read_backup_manifest(&backup_dir)?;
    restore_backup_files(&home, &backup_dir)?;

    let previous_takeover_enabled = manifest
        .cc_switch_state
        .as_ref()
        .is_some_and(|previous| previous.previous_takeover_enabled);
    if let Some(previous) = manifest.cc_switch_state.as_ref() {
        let state_errors = restore_cc_switch_state(state, &app_type, previous);
        if !state_errors.is_empty() {
            return Err(state_errors.join("；"));
        }
    }

    drop(switch_guard);
    if previous_takeover_enabled {
        state
            .proxy_service
            .set_takeover_for_app(app_type.as_str(), true)
            .await
            .map_err(|error| format!("恢复 Codex 代理接管失败：{error}"))?;
    }

    remove_backup_dir(&backup_dir)?;
    Ok(status_from_home(&home))
}

async fn capture_cc_switch_state(
    state: &AppState,
    app_type: &AppType,
) -> Result<BackupCcSwitchState, String> {
    Ok(BackupCcSwitchState {
        previous_clawkit_provider: state
            .db
            .get_provider_by_id(PROVIDER_ID, app_type.as_str())
            .map_err(|error| error.to_string())?,
        previous_local_current: crate::settings::get_current_provider(app_type),
        previous_db_current: state
            .db
            .get_current_provider(app_type.as_str())
            .map_err(|error| error.to_string())?,
        previous_takeover_enabled: state
            .db
            .get_proxy_config_for_app(app_type.as_str())
            .await
            .map_err(|error| error.to_string())?
            .enabled,
    })
}

fn restore_cc_switch_state(
    state: &AppState,
    app_type: &AppType,
    previous: &BackupCcSwitchState,
) -> Vec<String> {
    let mut errors = Vec::new();
    let provider_result = match previous.previous_clawkit_provider.as_ref() {
        Some(provider) => state.db.save_provider(app_type.as_str(), provider),
        None => state.db.delete_provider(app_type.as_str(), PROVIDER_ID),
    };
    if let Err(error) = provider_result {
        errors.push(format!("恢复 CC Switch 供应商失败：{error}"));
    }

    let db_current_result = match previous.previous_db_current.as_deref() {
        Some(provider_id) => state
            .db
            .set_current_provider(app_type.as_str(), provider_id),
        None => state.db.clear_current_provider(app_type.as_str()),
    };
    if let Err(error) = db_current_result {
        errors.push(format!("恢复数据库当前供应商失败：{error}"));
    }

    if let Err(error) =
        crate::settings::set_current_provider(app_type, previous.previous_local_current.as_deref())
    {
        errors.push(format!("恢复本地当前供应商失败：{error}"));
    }
    errors
}

async fn restore_takeover_if_needed(
    state: &AppState,
    previous: &BackupCcSwitchState,
) -> Result<(), String> {
    if previous.previous_takeover_enabled {
        state
            .proxy_service
            .set_takeover_for_app(AppType::Codex.as_str(), true)
            .await
            .map_err(|error| format!("恢复 Codex 代理接管失败：{error}"))?;
    }
    Ok(())
}

async fn rollback_failed_apply(
    error: String,
    home: &Path,
    backup_dir: &Path,
    state: &AppState,
    app_type: &AppType,
    previous: &BackupCcSwitchState,
    switch_guard: tokio::sync::OwnedMutexGuard<()>,
) -> Result<ClawkitCodexConfigurationStatus, String> {
    let mut rollback_errors = Vec::new();
    if let Err(restore_error) = restore_backup_files(home, backup_dir) {
        rollback_errors.push(format!("恢复 Codex 文件失败：{restore_error}"));
    }
    rollback_errors.extend(restore_cc_switch_state(state, app_type, previous));
    drop(switch_guard);
    if let Err(restore_error) = restore_takeover_if_needed(state, previous).await {
        rollback_errors.push(restore_error);
    }
    if rollback_errors.is_empty() {
        if let Err(cleanup_error) = std::fs::remove_dir_all(backup_dir) {
            log::warn!(
                "Unable to remove rolled-back ClawKit Codex backup {}: {cleanup_error}",
                backup_dir.display()
            );
        }
    }
    Err(combine_rollback_errors(error, rollback_errors))
}

fn combine_rollback_error(error: String, rollback_error: Option<String>) -> String {
    rollback_error.map_or(error.clone(), |rollback_error| {
        format!("{error}；同时{rollback_error}")
    })
}

fn combine_rollback_errors(error: String, rollback_errors: Vec<String>) -> String {
    if rollback_errors.is_empty() {
        error
    } else {
        format!("{error}；同时{}", rollback_errors.join("；"))
    }
}
