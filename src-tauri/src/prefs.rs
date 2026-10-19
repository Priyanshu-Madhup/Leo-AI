//! Permissions the user has chosen to relax. Nothing here is on by default.
//!
//! Leo normally shows an approval card before anything that changes something.
//! The user can switch that off for ONE narrow case: creating or updating a
//! calendar event for themselves. It is a real setting (Settings, or asked for
//! in chat and confirmed once on a card), not something the model can decide.
//! Deleting an event and any event that would invite other people still ask.

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Default)]
pub struct Prefs {
    calendar_auto: AtomicBool,
}

impl Prefs {
    pub fn calendar_auto(&self) -> bool {
        self.calendar_auto.load(Ordering::SeqCst)
    }

    pub fn set_calendar_auto(&self, enabled: bool) {
        self.calendar_auto.store(enabled, Ordering::SeqCst);
    }
}

/// The Settings switch. The screen keeps the saved value and sends it at start-up.
#[tauri::command]
pub fn set_calendar_auto_approve(prefs: State<'_, Prefs>, enabled: bool) {
    prefs.set_calendar_auto(enabled);
}

/// Sets the preference from the chat (after the user confirmed on a card) and
/// tells the screen so it can keep the switch and the saved value in step.
pub fn apply_calendar_auto_from_chat(app: &AppHandle, enabled: bool) {
    app.state::<Prefs>().set_calendar_auto(enabled);
    let _ = app.emit("prefs://calendar", enabled);
}

/// True when this call needs no approval card because the user turned that off
/// for plain calendar writes.
pub fn calendar_write_skips_approval(app: &AppHandle, name: &str, args: &str) -> bool {
    name == "google__manage_event" && app.state::<Prefs>().calendar_auto() && is_plain_event_write(args)
}

/// Create or update only, and nobody else is invited.
fn is_plain_event_write(args: &str) -> bool {
    let v: Value = serde_json::from_str(args).unwrap_or_default();
    let action = v["action"].as_str().unwrap_or("").trim().to_lowercase();
    if !matches!(action.as_str(), "create" | "update") {
        return false;
    }
    match v.get("attendees") {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.trim().is_empty(),
        Some(Value::Array(list)) => list.is_empty(),
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::is_plain_event_write;

    #[test]
    fn plain_event_writes_skip_approval() {
        assert!(is_plain_event_write(r#"{"action":"create","summary":"Gym"}"#));
        assert!(is_plain_event_write(r#"{"action":"Update","event_id":"x","attendees":[]}"#));
        assert!(is_plain_event_write(r#"{"action":"create","attendees":""}"#));
    }

    #[test]
    fn deleting_or_inviting_still_asks() {
        assert!(!is_plain_event_write(r#"{"action":"delete","event_id":"x"}"#));
        assert!(!is_plain_event_write(r#"{"action":"create","attendees":["a@b.com"]}"#));
        assert!(!is_plain_event_write(r#"{"action":"create","attendees":"a@b.com"}"#));
        assert!(!is_plain_event_write(r#"{"action":"rsvp"}"#));
        assert!(!is_plain_event_write("not json"));
    }
}
