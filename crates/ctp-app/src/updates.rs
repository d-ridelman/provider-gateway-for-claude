//! Updates from GitHub Releases (tauri-plugin-updater).
//!
//! The release carries `latest.json` and the NSIS installer signed with the project's
//! updater key; the app installs only a file whose signature matches the public key
//! built into it (`plugins.updater.pubkey` in tauri.conf.json).

use crate::tr;
use serde::Serialize;
use std::time::Duration;
use tauri::Manager;
use tauri_plugin_updater::UpdaterExt;

/// While the app lives in the tray it checks again this often.
const PERIOD: Duration = Duration::from_secs(24 * 60 * 60);
/// First check a little after start, not during it.
const FIRST_DELAY: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub notes: Option<String>,
}

/// The newer release, if there is one. `Err` — the check itself failed (no network,
/// no release yet, bad signature data).
pub async fn check(app: &tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    let updater = app.updater().map_err(|exc| exc.to_string())?;
    let update = updater.check().await.map_err(|exc| {
        tr!(
            "Не удалось проверить обновления: {exc}",
            "Could not check for updates: {exc}"
        )
    })?;
    Ok(update.map(|update| UpdateInfo {
        version: update.version.clone(),
        notes: update.body.clone(),
    }))
}

/// Downloads, verifies and runs the installer, then restarts into the new version.
/// The gateway stops with the app; the installer runs in passive mode.
pub async fn install(app: &tauri::AppHandle) -> Result<(), String> {
    let updater = app.updater().map_err(|exc| exc.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|exc| exc.to_string())?
        .ok_or_else(|| tr!("Обновлений нет", "No update available"))?;
    tracing::info!(version = %update.version, "installing update");
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|exc| {
            tr!(
                "Обновление не установлено: {exc}",
                "Update not installed: {exc}"
            )
        })?;
    app.restart();
}

/// Background checks: shortly after start, then once a day, if the user keeps them on.
pub fn spawn_auto_check(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_DELAY).await;
        loop {
            let enabled = {
                let shared = app.state::<crate::state::AppShared>();
                let config = shared.config.lock().unwrap_or_else(|p| p.into_inner());
                config.auto_update
            };
            if enabled {
                match check(&app).await {
                    Ok(found) => {
                        let shared = app.state::<crate::state::AppShared>();
                        *shared.update.lock().unwrap_or_else(|p| p.into_inner()) = found;
                    }
                    Err(exc) => tracing::info!(error = %exc, "update check skipped"),
                }
            }
            tokio::time::sleep(PERIOD).await;
        }
    });
}
