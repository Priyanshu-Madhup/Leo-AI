//! Self-update from GitHub Releases.
//!
//! On every launch of an installed (release) build, Leo asks GitHub for the
//! newest release. If it is newer, the signed installer is downloaded while
//! the window plays its update animation, run quietly once the animation has
//! finished, and the app restarts into the new version. Settings also has a
//! "Check for updates" button (`check_for_updates`). Updates are verified
//! against the public key in `tauri.conf.json`; the release workflow publishes
//! `latest.json` next to the installer.
//!
//! Everything the updater does is written to `updater.log` in the app's
//! config folder, because a background update has no other place to fail
//! visibly.

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

/// One try at reaching GitHub may take this long (slow connections, redirects
/// through GitHub's file servers); a failed try is repeated a couple of times.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const CHECK_ATTEMPTS: u32 = 3;
/// The update animation in the window runs this long; the installer waits for
/// it so the flight is never cut short by the app closing.
const ANIMATION: Duration = Duration::from_secs(15);
const MAX_LOG_BYTES: u64 = 100_000;

fn log_file(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join("updater.log"))
}

fn log(app: &AppHandle, message: &str) {
    eprintln!("[updater] {message}");
    let Some(file) = log_file(app) else { return };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Keep the file small: start over once it grows too big.
    if std::fs::metadata(&file).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
        let _ = std::fs::remove_file(&file);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
        let _ = writeln!(f, "{}  {message}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    }
}

pub fn check_on_launch(app: &AppHandle) {
    // Development builds are never replaced by a release.
    if cfg!(debug_assertions) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        log(&app, &format!("launch check, installed version {}", app.package_info().version));
        match find_update(&app).await {
            Ok(Some(update)) => apply(&app, update).await,
            Ok(None) => log(&app, "up to date"),
            Err(err) => log(&app, &format!("check failed: {err}")),
        }
    });
}

async fn find_update(app: &AppHandle) -> Result<Option<Update>, String> {
    let mut last = String::new();
    for attempt in 1..=CHECK_ATTEMPTS {
        let updater = app.updater().map_err(|e| e.to_string())?;
        match tokio::time::timeout(CHECK_TIMEOUT, updater.check()).await {
            Ok(Ok(found)) => return Ok(found),
            Ok(Err(e)) => last = e.to_string(),
            Err(_) => last = "timed out".to_string(),
        }
        log(app, &format!("check attempt {attempt} of {CHECK_ATTEMPTS} failed: {last}"));
        if attempt < CHECK_ATTEMPTS {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
    Err(last)
}

/// Plays out the update: announce it, download, wait for the animation, install.
async fn apply(app: &AppHandle, update: Update) {
    log(app, &format!("update found: {}", update.version));
    let started = Instant::now();
    let _ = app.emit("app://updating", update.version.clone());
    match install(app, &update, started).await {
        Ok(()) => {
            log(app, "installed, restarting");
            app.restart()
        }
        Err(err) => {
            log(app, &format!("update failed: {err}"));
            // Let the window end the flight and show the launch animation instead.
            let _ = app.emit("app://update-failed", ());
        }
    }
}

/// Downloads at once (the animation covers the wait), then holds until the
/// animation has played out before installing, which closes the app.
async fn install(app: &AppHandle, update: &Update, started: Instant) -> Result<(), String> {
    let bytes = update
        .download(|_chunk, _total| {}, || {})
        .await
        .map_err(|e| format!("download: {e}"))?;
    log(app, &format!("downloaded {} bytes in {:.1}s", bytes.len(), started.elapsed().as_secs_f32()));
    tokio::time::sleep(ANIMATION.saturating_sub(started.elapsed())).await;
    update.install(bytes).map_err(|e| format!("install: {e}"))
}

#[derive(Serialize)]
pub struct UpdateStatus {
    /// "current", "updating" or "error".
    state: &'static str,
    message: String,
}

/// The Settings button: checks now and, if there is a newer version, starts
/// the update. Returns at once; the update itself runs in the background.
#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> UpdateStatus {
    let version = app.package_info().version.to_string();
    if cfg!(debug_assertions) {
        return UpdateStatus {
            state: "current",
            message: format!("This is a development build ({version}); updates only apply to the installed app."),
        };
    }
    log(&app, &format!("manual check, installed version {version}"));
    match find_update(&app).await {
        Ok(Some(update)) => {
            let message = format!("Version {} is available. Updating now…", update.version);
            let app = app.clone();
            tauri::async_runtime::spawn(async move { apply(&app, update).await });
            UpdateStatus { state: "updating", message }
        }
        Ok(None) => {
            log(&app, "up to date");
            UpdateStatus { state: "current", message: format!("You're up to date (version {version}).") }
        }
        Err(err) => {
            log(&app, &format!("check failed: {err}"));
            UpdateStatus {
                state: "error",
                message: format!("Couldn't check for updates: {err}. Details are in updater.log in Leo's settings folder."),
            }
        }
    }
}
