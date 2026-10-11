//! Facts every agent shares for the whole app session: today's date and time
//! and where the user is (looked up from their IP address).
//!
//! The first agent that needs one looks it up and it is stored here; every
//! other agent then reads the stored value (it is also written into each
//! agent's prompt), so nothing is looked up twice. The clock keeps running
//! from the stored moment, so a stored time is never stale.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use serde::Deserialize;

use crate::profile::Profile;

const LOCATION_URL: &str = "https://ipwho.is/";
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub city: String,
    pub region: String,
    pub country: String,
    pub timezone: String,
}

impl Place {
    pub fn describe(&self) -> String {
        let parts: Vec<&str> = [&self.city, &self.region, &self.country]
            .into_iter()
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        let mut text = parts.join(", ");
        if !self.timezone.is_empty() {
            text.push_str(&format!(" (time zone {})", self.timezone));
        }
        text
    }
}

#[derive(Default)]
pub struct SessionInfo {
    /// The moment the clock was first read, and the matching instant, so the
    /// current time is `moment + elapsed` without asking again.
    clock: Mutex<Option<(DateTime<Local>, Instant)>>,
    place: Mutex<Option<Place>>,
    /// What the user told Leo about themselves (profile.rs).
    profile: Mutex<Option<Profile>>,
    /// True while `place` came from the profile rather than the IP lookup.
    place_from_profile: Mutex<bool>,
    /// Set once the saved profile has been read for the agents.
    profile_loaded: AtomicBool,
}

impl SessionInfo {
    /// Stores the clock on first use; later calls reuse it.
    fn anchor(&self) -> (DateTime<Local>, Instant) {
        *self.clock.lock().unwrap().get_or_insert_with(|| (Local::now(), Instant::now()))
    }

    /// Current local date, time and zone, from the stored clock.
    pub fn now(&self) -> DateTime<Local> {
        let (moment, at) = self.anchor();
        moment + chrono::Duration::from_std(at.elapsed()).unwrap_or_default()
    }

    pub fn now_text(&self) -> String {
        self.now().format("%A, %-d %B %Y, %H:%M (UTC%:z)").to_string()
    }

    pub fn date_text(&self) -> String {
        self.now().format("%A, %-d %B %Y").to_string()
    }

    /// True only the first time: the caller then loads the saved profile.
    pub fn take_unloaded_flag(&self) -> bool {
        !self.profile_loaded.swap(true, Ordering::SeqCst)
    }

    /// Stores the user's profile for the agents. A location in it replaces the
    /// IP-based one (and is dropped again if the user clears it).
    pub fn set_profile(&self, profile: &Profile) {
        self.profile_loaded.store(true, Ordering::SeqCst);
        *self.profile.lock().unwrap() = if profile.is_empty() { None } else { Some(profile.clone()) };
        let mut place = self.place.lock().unwrap();
        let mut from_profile = self.place_from_profile.lock().unwrap();
        if !profile.location.trim().is_empty() {
            *place = Some(Place {
                city: profile.location.trim().to_string(),
                region: String::new(),
                country: String::new(),
                timezone: String::new(),
            });
            *from_profile = true;
        } else if *from_profile {
            *place = None;
            *from_profile = false;
        }
    }

    pub fn place(&self) -> Option<Place> {
        self.place.lock().unwrap().clone()
    }

    /// The stored location, or one lookup by IP address that is then stored.
    pub async fn location(&self) -> Result<Place, String> {
        if let Some(place) = self.place() {
            return Ok(place);
        }
        let found = lookup().await?;
        *self.place.lock().unwrap() = Some(found.clone());
        Ok(found)
    }

    /// Lines for an agent's prompt with what is already known, so it uses
    /// them instead of calling a tool.
    pub fn prompt_note(&self) -> String {
        let mut note = format!("Today is {}.", self.date_text());
        match self.place() {
            Some(p) => note.push_str(&format!(
                " The user's location (found from their connection, approximate) is {}. Use it directly for anything local; do not call get_location.",
                p.describe()
            )),
            None => note.push_str(" The user's location is not known yet: call get_location once if the request depends on where they are."),
        }
        note.push_str(" You already know the date; call current_datetime only for the time of day.");
        if let Some(p) = self.profile.lock().unwrap().as_ref() {
            let name = p.name.trim();
            if !name.is_empty() {
                note.push_str(&format!(" The user's name is {name}; use it naturally and sparingly, not in every reply."));
            }
            if !p.occupation.trim().is_empty() {
                note.push_str(&format!(" About them (their own words): {}.", p.occupation.trim()));
            }
            if !p.interests.trim().is_empty() {
                note.push_str(&format!(" Their interests: {}.", p.interests.trim()));
            }
        }
        note
    }
}

#[derive(Deserialize)]
struct Tz {
    #[serde(default)]
    id: String,
}

#[derive(Deserialize)]
struct IpInfo {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    message: String,
    #[serde(default)]
    city: String,
    #[serde(default)]
    region: String,
    #[serde(default)]
    country: String,
    timezone: Option<Tz>,
}

async fn lookup() -> Result<Place, String> {
    let info: IpInfo = crate::openrouter::client()
        .get(LOCATION_URL)
        .timeout(LOOKUP_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("Couldn't work out the location: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Couldn't read the location: {e}"))?;
    parse(info)
}

fn parse(info: IpInfo) -> Result<Place, String> {
    if !info.success || (info.city.is_empty() && info.country.is_empty()) {
        return Err(format!("Couldn't work out the location. {}", info.message).trim().to_string());
    }
    Ok(Place {
        city: info.city,
        region: info.region,
        country: info.country,
        timezone: info.timezone.map(|t| t.id).unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_is_stored_once_and_keeps_running() {
        let s = SessionInfo::default();
        let first = s.now();
        std::thread::sleep(Duration::from_millis(20));
        assert!(s.now() > first);
        assert_eq!(s.anchor().0, s.anchor().0);
    }

    #[test]
    fn prompt_note_switches_once_location_is_known() {
        let s = SessionInfo::default();
        assert!(s.prompt_note().contains("call get_location once"));
        *s.place.lock().unwrap() =
            Some(Place { city: "Delhi".into(), region: "Delhi".into(), country: "India".into(), timezone: "Asia/Kolkata".into() });
        let note = s.prompt_note();
        assert!(note.contains("Delhi, Delhi, India (time zone Asia/Kolkata)"));
        assert!(!note.contains("call get_location once"));
    }

    #[test]
    fn profile_reaches_the_prompt_and_replaces_the_ip_location() {
        let s = SessionInfo::default();
        s.set_profile(&Profile {
            name: "Priyanshu".into(),
            location: "Delhi, India".into(),
            interests: "chess".into(),
            ..Default::default()
        });
        let note = s.prompt_note();
        assert!(note.contains("The user's name is Priyanshu"));
        assert!(note.contains("Delhi, India"));
        assert!(note.contains("Their interests: chess"));
        assert!(!note.contains("call get_location once"));
        // The date of birth is never part of the prompt.
        s.set_profile(&Profile { name: "Priyanshu".into(), dob: "2001-05-12".into(), ..Default::default() });
        assert!(!s.prompt_note().contains("2001"));
        // Clearing the location drops the profile's place again.
        assert!(s.place().is_none());
        assert!(s.prompt_note().contains("call get_location once"));
    }

    #[test]
    fn failed_lookup_is_an_error() {
        let bad = IpInfo { success: false, message: "limit".into(), city: String::new(), region: String::new(), country: String::new(), timezone: None };
        assert!(parse(bad).is_err());
    }
}
