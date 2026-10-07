//! Silent self-update from GitHub Releases.
//!
//! On every launch of an installed (release) build, Leo asks GitHub for the
//! newest release. If it is newer, the signed installer is downloaded and run
//! quietly, and the app restarts into the new version. Nothing is shown except
//! a short "Updating" status. The release workflow publishes `latest.json`
//! next to the installer; updates are verified against the public key in
//! `tauri.conf.json`.

use std::time::Duration;

use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;

const CHECK_TIMEOUT: Duration = Duration::from_secs(15);

pub fn check_on_launch(app: &AppHandle) {
    // Development builds are never replaced by a release.
    if cfg!(debug_assertions) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = run(&app).await {
            eprintln!("update check failed: {err}");
        }
    });
}

async fn run(app: &AppHandle) -> Result<(), String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let checked = tokio::time::timeout(CHECK_TIMEOUT, updater.check())
        .await
        .map_err(|_| "timed out".to_string())?
        .map_err(|e| e.to_string())?;
    let Some(update) = checked else {
        return Ok(());
    };

    let _ = app.emit("app://updating", update.version.clone());
    update
        .download_and_install(|_chunk, _total| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    app.restart()
}
