use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, Position, Size, WebviewWindow, WindowEvent};

mod agent;
mod agents;
mod interact;
mod mcp;
mod memory;
mod openrouter;
mod orchestrator;
mod planner;
mod reflection;
mod summary;
mod tools;
mod types;
mod updater;
mod verifier;
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
                *WIDGET_POS.lock().unwrap() = Some((x as i32, y as i32));
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
    let _ = std::fs::write(file, serde_json::json!({ "x": x, "y": y }).to_string());
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

const WIDGET_SIZE: (f64, f64) = (160.0, 150.0);
const FULL_SIZE: (f64, f64) = (440.0, 700.0);

fn apply_mode(window: &WebviewWindow, mode: &str) -> tauri::Result<()> {
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
    window.set_size(Size::Logical(LogicalSize {
        width: FULL_SIZE.0,
        height,
    }))?;
    // The full view is a normal app window, so it gets a taskbar entry;
    // the widget stays out of the taskbar.
    window.set_skip_taskbar(false)?;
    set_glass(window, true);

    let x = ((screen.width as f64 - FULL_SIZE.0 * scale) / 2.0).round() as i32;
    let y = ((screen.height as f64 - height * scale) / 2.0).round() as i32;
    window.set_position(Position::Physical(PhysicalPosition { x, y }))?;
    window.show()?;
    let _ = window.set_focus();
    Ok(())
}

#[tauri::command]
fn set_blur(window: WebviewWindow, enabled: bool) {
    BLUR_ENABLED.store(enabled, Ordering::SeqCst);
    if FULL_VIEW.load(Ordering::SeqCst) {
        set_glass(&window, true);
    }
}

#[tauri::command]
fn set_mode(window: WebviewWindow, mode: String) -> Result<(), String> {
    // Routed through Rust rather than the JS setSize() API: with
    // decorations disabled (as this window has), Window.setSize() from the
    // frontend is known to silently no-op on some Tauri/Windows builds.
    apply_mode(&window, &mode).map_err(|e| e.to_string())
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
        .invoke_handler(tauri::generate_handler![
            agent::agent_run,
            agent::agent_cancel,
            agent::agent_reset,
            interact::agent_answer,
            memory::memory_configure,
            memory::memory_is_configured,
            web::web_configure,
            web::web_is_configured,
            mcp::mcp_status,
            mcp::mcp_set_inject,
            set_mode,
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
            if let Err(err) = apply_mode(&window, "widget") {
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
