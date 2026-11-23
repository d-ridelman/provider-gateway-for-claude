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

/// While the app lives in the tray it checks again this often after a successful check.
const PERIOD: Duration = Duration::from_secs(24 * 60 * 60);
/// How often the loop wakes up: retries after a failed check (no network) and picks up
/// auto-update being turned on without waiting a whole day.
const TICK: Duration = Duration::from_secs(60 * 60);
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
    // До установки: на Windows установщик запускается, и сам плагин завершает процесс, до
    // строки после `download_and_install` дело не доходит. Если русская копия Claude ещё
    // открыта, её след в реестре иначе остался бы до следующего старта.
    crate::claude::repair_registry_on_exit(app);
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

/// A check is due if none succeeded yet or the last success is a day old.
fn due(since_ok: Option<Duration>) -> bool {
    since_ok.is_none_or(|elapsed| elapsed >= PERIOD)
}

/// Background checks: shortly after start, then once a day, if the user keeps them on.
pub fn spawn_auto_check(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_DELAY).await;
        let mut last_ok: Option<std::time::Instant> = None;
        loop {
            let enabled = {
                let shared = app.state::<crate::state::AppShared>();
                let config = shared.config.lock().unwrap_or_else(|p| p.into_inner());
                config.auto_update
            };
            if enabled && due(last_ok.map(|at| at.elapsed())) {
                match check(&app).await {
                    Ok(found) => {
                        last_ok = Some(std::time::Instant::now());
                        let shared = app.state::<crate::state::AppShared>();
                        *shared.update.lock().unwrap_or_else(|p| p.into_inner()) = found;
                    }
                    Err(exc) => {
                        tracing::info!(error = %exc, "update check failed, retry in an hour")
                    }
                }
            }
            tokio::time::sleep(TICK).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_is_due_until_a_day_after_success() {
        assert!(due(None));
        assert!(!due(Some(TICK)));
        assert!(due(Some(PERIOD)));
    }
}
