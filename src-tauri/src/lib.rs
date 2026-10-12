use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize, Position, Size, WebviewWindow, WindowEvent};

mod agent;
mod agents;
mod history;
mod interact;
mod mcp;
mod memory;
mod openrouter;
mod orchestrator;
mod planner;
mod profile;
mod reflection;
mod session;
mod summary;
mod tools;
mod types;
mod updater;
mod verifier;
mod vision;
mod web;

#[cfg(debug_assertions)]
mod e2e;

fn position_top_right_inner(window: &WebviewWindow) -> tauri::Result<()> {
    let monitor = window
        .current_monitor()?
        .or(window.primary_monitor()?)
        .ok_or_else(|| tauri::Error::WindowNotFound)?;

    let scale = monitor.scale_factor();
    let screen_size = monitor.size();
    let win_size = window.outer_size()?;

    // Flush against the right edge, but nudged down a bit: most maximized
    // windows put their close/minimize/restore buttons in that exact top
    // corner, and this window accepts clicks across its whole (mostly
    // transparent) rectangle, so sitting at y=0 was blocking them.
    let top_clearance = (34.0 * scale).round() as i32;
    let x = screen_size.width as i32 - win_size.width as i32;
    let y = top_clearance;

    window.set_position(Position::Physical(PhysicalPosition { x, y }))?;
    window.show()?;
    let _ = window.set_focus();
    Ok(())
}

/// Frosted-glass window: Windows acrylic blurs whatever is behind the window.
/// Only the full chat view gets it; the small orb widget stays fully
/// transparent so no rectangle shows around it. Shadow gives the borderless
/// window its rounded corners on Windows 11.
/// Whether the user wants the blur at all (Settings), and whether the full
/// chat view is currently showing.
static BLUR_ENABLED: AtomicBool = AtomicBool::new(true);
static FULL_VIEW: AtomicBool = AtomicBool::new(true);

fn set_glass(window: &WebviewWindow, on: bool) {
    FULL_VIEW.store(on, Ordering::SeqCst);
    #[cfg(target_os = "windows")]
    {
        use window_vibrancy::{apply_acrylic, clear_acrylic};
        if on && BLUR_ENABLED.load(Ordering::SeqCst) {
            // Nearly clear neutral tint: the blur does the work.
            if let Err(err) = apply_acrylic(window, Some((16, 16, 18, 6))) {
                eprintln!("acrylic unavailable: {err}");
            }
        } else {
            let _ = clear_acrylic(window);
        }
    }
    let _ = window.set_shadow(on);
}

// ---------- where the minimised orb lives ----------
// The orb can be dragged anywhere. Its position (top-left, physical pixels) is
// tracked while it is the minimised widget, kept on disk, and used the next
// time it minimises. Without one it goes to the top-right corner.

static WIDGET_POS: Mutex<Option<(i32, i32)>> = Mutex::new(None);
static WIDGET_POS_LOADED: AtomicBool = AtomicBool::new(false);

fn widget_pos_file(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join("widget-position.json"))
}

fn widget_pos(app: &AppHandle) -> Option<(i32, i32)> {
    if !WIDGET_POS_LOADED.swap(true, Ordering::SeqCst) {
        let saved = widget_pos_file(app)
            .and_then(|file| std::fs::read_to_string(file).ok())
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
        if let Some(v) = saved {
            if let (Some(x), Some(y)) = (v["x"].as_i64(), v["y"].as_i64()) {
                let (mut x, mut y) = (x as i32, y as i32);
                // A spot saved for the old, bigger window: move it so the orb
                // stays exactly where it was on screen.
                if v["v"].as_i64() != Some(2) {
                    let scale = app
                        .get_webview_window("main")
                        .and_then(|w| w.scale_factor().ok())
                        .unwrap_or(1.0);
                    x += ((OLD_WIDGET_ORB_CENTER.0 - ORB_CENTER_IN_WIDGET.0) * scale).round() as i32;
                    y += ((OLD_WIDGET_ORB_CENTER.1 - ORB_CENTER_IN_WIDGET.1) * scale).round() as i32;
                }
                *WIDGET_POS.lock().unwrap() = Some((x, y));
            }
        }
    }
    *WIDGET_POS.lock().unwrap()
}

fn save_widget_pos(app: &AppHandle) {
    let Some((x, y)) = *WIDGET_POS.lock().unwrap() else { return };
    let Some(file) = widget_pos_file(app) else { return };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(file, serde_json::json!({ "x": x, "y": y, "v": 2 }).to_string());
}

/// Keeps a restored position on a screen that still exists (a monitor may
/// have been unplugged or the resolution changed since it was saved).
fn clamp_to_screen(window: &WebviewWindow, x: i32, y: i32) -> tauri::Result<(i32, i32)> {
    let monitors = window.available_monitors()?;
    let hit = monitors.iter().find(|m| {
        let p = m.position();
        let s = m.size();
        x >= p.x && x < p.x + s.width as i32 && y >= p.y && y < p.y + s.height as i32
    });
    let Some(m) = hit.or(monitors.first()) else { return Ok((x, y)) };
    let scale = m.scale_factor();
    let (w, h) = ((WIDGET_SIZE.0 * scale).round() as i32, (WIDGET_SIZE.1 * scale).round() as i32);
    let (p, s) = (m.position(), m.size());
    Ok((
        x.clamp(p.x, (p.x + s.width as i32 - w).max(p.x)),
        y.clamp(p.y, (p.y + s.height as i32 - h).max(p.y)),
    ))
}

fn place_widget(window: &WebviewWindow, saved: Option<(i32, i32)>) -> tauri::Result<()> {
    let Some((x, y)) = saved else {
        return position_top_right_inner(window);
    };
    let (x, y) = clamp_to_screen(window, x, y)?;
    window.set_position(Position::Physical(PhysicalPosition { x, y }))?;
    window.show()?;
    let _ = window.set_focus();
    Ok(())
}

/// The orb window: the orb canvas is 110 px, so this is just a 5 px margin
/// around it (it used to be 160x150, which left a big empty square on screen).
const WIDGET_SIZE: (f64, f64) = (120.0, 120.0);
/// The old size, only for moving a saved position to the new window.
const OLD_WIDGET_ORB_CENTER: (f64, f64) = (80.0, 65.0);
const FULL_SIZE: (f64, f64) = (440.0, 700.0);

/// `quiet` (opening the chat inside the grow animation): no blur or shadow yet,
/// because they fill the whole window at once; `glass_on` adds them at the end.
fn apply_mode(window: &WebviewWindow, mode: &str, quiet: bool) -> tauri::Result<()> {
    if mode == "widget" {
        // Read before resizing: window events during the switch must not
        // overwrite where the user last left the orb.
        let saved = widget_pos(window.app_handle());
        window.set_size(Size::Logical(LogicalSize {
            width: WIDGET_SIZE.0,
            height: WIDGET_SIZE.1,
        }))?;
        window.set_skip_taskbar(true)?;
        set_glass(window, false);
        return place_widget(window, saved);
    }

    // Leaving widget mode: keep the orb's spot for next time.
    if !FULL_VIEW.load(Ordering::SeqCst) {
        save_widget_pos(window.app_handle());
    }

    let monitor = window
        .current_monitor()?
        .or(window.primary_monitor()?)
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    let scale = monitor.scale_factor();
    let screen = monitor.size();

    // Never taller than the screen, leaving room for the taskbar.
    let height = FULL_SIZE.1.min(screen.height as f64 / scale - 80.0);
    if !quiet {
        window.set_size(Size::Logical(LogicalSize {
            width: FULL_SIZE.0,
            height,
        }))?;
    }
    // The full view is a normal app window, so it gets a taskbar entry;
    // the widget stays out of the taskbar.
    window.set_skip_taskbar(false)?;
    if quiet {
        FULL_VIEW.store(true, Ordering::SeqCst);
    } else {
        set_glass(window, true);
    }

    // Centred on the screen the orb is on.
    let origin = monitor.position();
    let x = origin.x + ((screen.width as f64 - FULL_SIZE.0 * scale) / 2.0).round() as i32;
    let y = origin.y + ((screen.height as f64 - height * scale) / 2.0).round() as i32;
    if quiet {
        // One call for size and place: the bright point must not jump around
        // while the window grows underneath it.
        move_window(window, x, y, (FULL_SIZE.0 * scale).round() as u32, (height * scale).round() as u32)?;
    } else {
        window.set_position(Position::Physical(PhysicalPosition { x, y }))?;
    }
    window.show()?;
    let _ = window.set_focus();
    Ok(())
}

/// Where the minimised orb's window goes: where it was last left (kept on a
/// screen that still exists), or the top-right corner.
fn widget_target(window: &WebviewWindow, saved: Option<(i32, i32)>) -> tauri::Result<(i32, i32)> {
    if let Some((x, y)) = saved {
        return clamp_to_screen(window, x, y);
    }
    let monitor = window
        .current_monitor()?
        .or(window.primary_monitor()?)
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    let scale = monitor.scale_factor();
    let width = (WIDGET_SIZE.0 * scale).round() as i32;
    let p = monitor.position();
    Ok((p.x + monitor.size().width as i32 - width, p.y + (34.0 * scale).round() as i32))
}

/// Where the orb sits inside the 160x150 widget window (logical pixels); the
/// bright point flies to this spot and the orb grows there.
const ORB_CENTER_IN_WIDGET: (f64, f64) = (60.0, 60.0);
/// How long the point takes to fly to the orb's place.
const FLIGHT_SECONDS: f64 = 0.6 * 1.15; // 1.15 = SLOW in main.ts

fn ease_in_out(t: f64) -> f64 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Switches the blur behind the window off for the minimise animation. The
/// blur always fills the whole window, so it cannot shut like the rest.
/// (Unlike `set_glass` this leaves `FULL_VIEW` alone, so the flight below is
/// not mistaken for the user dragging the orb.)
#[tauri::command]
fn glass_off(window: WebviewWindow) {
    #[cfg(target_os = "windows")]
    {
        let _ = window_vibrancy::clear_acrylic(&window);
    }
    let _ = window.set_shadow(false);
}

/// Minimising, after the window has closed to a point at its centre: the
/// window becomes the small orb window around that same point (one call, so the
/// point does not move). The point is then at the orb's centre of the small
/// window. This is the reverse of what opening does last.
#[tauri::command]
fn shrink_to_orb_window(window: WebviewWindow) -> Result<(), String> {
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let pos = window.outer_position().map_err(|e| e.to_string())?;
    let size = window.outer_size().map_err(|e| e.to_string())?;
    let centre = (pos.x as f64 + size.width as f64 / 2.0, pos.y as f64 + size.height as f64 / 2.0);
    let (w, h) = ((WIDGET_SIZE.0 * scale).round() as u32, (WIDGET_SIZE.1 * scale).round() as u32);
    move_window(
        &window,
        (centre.0 - ORB_CENTER_IN_WIDGET.0 * scale).round() as i32,
        (centre.1 - ORB_CENTER_IN_WIDGET.1 * scale).round() as i32,
        w,
        h,
    )
    .map_err(|e| e.to_string())
}

/// The small orb window, showing a bright point at the orb's centre, carries
/// that point across the screen to where the orb lives (the reverse of
/// `fly_to_center`). The window keeps its size while it flies.
#[tauri::command]
async fn fly_to_orb(window: WebviewWindow) -> Result<(), String> {
    fly(window).await.map_err(|e| e.to_string())
}

async fn fly(window: WebviewWindow) -> tauri::Result<()> {
    let saved = widget_pos(window.app_handle());
    let scale = window.scale_factor()?;
    let pos = window.outer_position()?;
    let size = window.outer_size()?;
    let (to_x, to_y) = widget_target(&window, saved)?;

    // Where the point is now (the middle of the window), and where the orb's
    // centre will be.
    let dot = (pos.x as f64 + size.width as f64 / 2.0, pos.y as f64 + size.height as f64 / 2.0);
    let goal = (to_x as f64 + ORB_CENTER_IN_WIDGET.0 * scale, to_y as f64 + ORB_CENTER_IN_WIDGET.1 * scale);
    glide(&window, goal.0 - dot.0, goal.1 - dot.1).await
}

/// Opening the chat: the orb has shrunk to a point inside the small window;
/// the window carries it to the middle of the screen.
#[tauri::command]
async fn fly_to_center(window: WebviewWindow) -> Result<(), String> {
    fly_center(window).await.map_err(|e| e.to_string())
}

async fn fly_center(window: WebviewWindow) -> tauri::Result<()> {
    // The orb's own spot is kept now; from here on the window moving is not
    // the user dragging the orb, so it is not tracked as one.
    let _ = widget_pos(window.app_handle());
    save_widget_pos(window.app_handle());
    FULL_VIEW.store(true, Ordering::SeqCst);

    let monitor = window
        .current_monitor()?
        .or(window.primary_monitor()?)
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    let scale = window.scale_factor()?;
    let pos = window.outer_position()?;
    let (origin, area) = (monitor.position(), monitor.size());

    let dot = (pos.x as f64 + ORB_CENTER_IN_WIDGET.0 * scale, pos.y as f64 + ORB_CENTER_IN_WIDGET.1 * scale);
    let goal = (origin.x as f64 + area.width as f64 / 2.0, origin.y as f64 + area.height as f64 / 2.0);
    glide(&window, goal.0 - dot.0, goal.1 - dot.1).await
}

/// Moves the window (size unchanged) by `dx`, `dy` physical pixels with a
/// smooth start and stop.
async fn glide(window: &WebviewWindow, dx: f64, dy: f64) -> tauri::Result<()> {
    let pos = window.outer_position()?;
    let size = window.outer_size()?;
    let started = std::time::Instant::now();
    loop {
        let t = (started.elapsed().as_secs_f64() / FLIGHT_SECONDS).min(1.0);
        let e = ease_in_out(t);
        move_window(
            window,
            pos.x + (dx * e).round() as i32,
            pos.y + (dy * e).round() as i32,
            size.width,
            size.height,
        )?;
        if t >= 1.0 {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(4)).await;
    }
}

/// Blur and shadow back on once the chat has finished opening.
#[tauri::command]
fn glass_on(window: WebviewWindow) {
    set_glass(&window, true);
}

/// Moves and resizes in ONE system call so the two never show out of step.
fn move_window(window: &WebviewWindow, x: i32, y: i32, w: u32, h: u32) -> tauri::Result<()> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER};
        if let Ok(hwnd) = window.hwnd() {
            // SAFETY: the handle belongs to this live window.
            unsafe { SetWindowPos(hwnd.0 as _, std::ptr::null_mut(), x, y, w as i32, h as i32, SWP_NOZORDER | SWP_NOACTIVATE) };
            return Ok(());
        }
    }
    window.set_size(Size::Physical(PhysicalSize { width: w, height: h }))?;
    window.set_position(Position::Physical(PhysicalPosition { x, y }))
}

#[tauri::command]
fn set_blur(window: WebviewWindow, enabled: bool) {
    BLUR_ENABLED.store(enabled, Ordering::SeqCst);
    if FULL_VIEW.load(Ordering::SeqCst) {
        set_glass(&window, true);
    }
}

#[tauri::command]
async fn set_mode(window: WebviewWindow, mode: String, quiet: Option<bool>) -> Result<(), String> {
    // Routed through Rust rather than the JS setSize() API: with
    // decorations disabled (as this window has), Window.setSize() from the
    // frontend is known to silently no-op on some Tauri/Windows builds.
    apply_mode(&window, &mode, quiet == Some(true)).map_err(|e| e.to_string())
}

/// Sets (or, when empty, clears) the global shortcut that brings Leo to the
/// front from anywhere. Only one shortcut is ever registered.
#[tauri::command]
fn set_shortcut(app: AppHandle, shortcut: String) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
    let gs = app.global_shortcut();
    gs.unregister_all().map_err(|e| e.to_string())?;
    let shortcut = shortcut.trim();
    if shortcut.is_empty() {
        return Ok(());
    }
    gs.on_shortcut(shortcut, |app, _shortcut, event| {
        if event.state() != ShortcutState::Pressed {
            return;
        }
        eprintln!("[shortcut] pressed");
        if let Some(window) = app.get_webview_window("main") {
            // Brought back from the taskbar: just show it. Otherwise the
            // frontend flips between the orb widget and the full chat.
            let restored = window.is_minimized().unwrap_or(false) || !window.is_visible().unwrap_or(true);
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
            eprintln!("[shortcut] restored={restored}");
            let _ = app.emit("shortcut://pressed", restored);
        }
    })
    .map_err(|_| "That shortcut isn't valid or is already used by another app.".to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(agent::AgentState::default())
        .manage(memory::MemoryState::default())
        .manage(mcp::McpManager::default())
        .manage(interact::InteractState::default())
        .manage(web::WebState::default())
        .manage(session::SessionInfo::default())
        .manage(history::Db::default())
        .invoke_handler(tauri::generate_handler![
            agent::agent_run,
            agent::agent_cancel,
            vision::screen_prepared,
            history::history_list,
            profile::profile_get,
            profile::profile_save,
            agent::history_open,
            agent::history_delete,
            agent::history_current,
            agent::agent_reset,
            interact::agent_answer,
            memory::memory_configure,
            memory::memory_is_configured,
            web::web_configure,
            web::web_is_configured,
            mcp::mcp_status,
            mcp::mcp_set_inject,
            set_mode,
            glass_off,
            glass_on,
            shrink_to_orb_window,
            fly_to_orb,
            fly_to_center,
            set_blur,
            set_shortcut,
            updater::check_for_updates
        ])
        .setup(|app| {
            // Debug builds only: a headless timing run, switched on by env vars.
            #[cfg(debug_assertions)]
            if e2e::active() {
                e2e::run(app.handle());
                return Ok(());
            }
            let window = app
                .get_webview_window("main")
                .expect("main window must exist");
            if let Err(err) = apply_mode(&window, "widget", false) {
                eprintln!("failed to position window: {err}");
                let _ = window.show();
            }
            // Track the orb while it is minimised, and save its spot on exit.
            let handle = app.handle().clone();
            window.on_window_event(move |event| match event {
                WindowEvent::Moved(pos) if !FULL_VIEW.load(Ordering::SeqCst) => {
                    *WIDGET_POS.lock().unwrap() = Some((pos.x, pos.y));
                }
                WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed => save_widget_pos(&handle),
                _ => {}
            });
            // First thing: pick up a newer release if there is one.
            updater::check_on_launch(app.handle());
            mcp::ensure_default_config(app.handle());
            mcp::start_all(app.handle());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod glide_tests {
    use super::ease_in_out;

    #[test]
    fn flight_curve_runs_from_zero_to_one_without_going_back() {
        assert!(ease_in_out(0.0).abs() < 1e-9);
        assert!((ease_in_out(1.0) - 1.0).abs() < 1e-9);
        let mut last = 0.0;
        for i in 1..=100 {
            let v = ease_in_out(i as f64 / 100.0);
            assert!(v >= last, "curve went backwards at {i}");
            last = v;
        }
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-9);
    }
}
